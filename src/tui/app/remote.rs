use super::{
    ctrl_bracket_fallback_to_esc, parse_rate_limit_error, spawn_in_new_terminal, App,
    DisplayMessage, ProcessingStatus, SendAction,
};
use crate::bus::BusEvent;
use crate::message::ToolCall;
use crate::protocol::ServerEvent;
use crate::tool::selfdev::ReloadContext;
use crate::tui::backend::RemoteConnection;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEvent};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct RemoteRunState {
    pub reconnect_attempts: u32,
    pub disconnect_msg_idx: Option<usize>,
    pub disconnect_start: Option<Instant>,
}

pub(super) enum ConnectOutcome {
    Connected(RemoteConnection),
    Retry,
    Quit,
}

pub(super) enum PostConnectOutcome {
    Ready,
    Quit,
}

pub(super) enum RemoteEventOutcome {
    Continue,
    Reconnect,
}

pub(super) async fn handle_tick(app: &mut App, remote: &mut RemoteConnection) {
    if app.stream_buffer.should_flush() {
        if let Some(chunk) = app.stream_buffer.flush() {
            app.streaming_text.push_str(&chunk);
        }
    }

    let _ = check_debug_command(app, remote).await;

    if let Some(reset_time) = app.rate_limit_reset {
        if Instant::now() >= reset_time {
            app.rate_limit_reset = None;
            if !app.is_processing {
                if let Some(pending) = app.rate_limit_pending_message.clone() {
                    app.push_display_message(DisplayMessage::system(format!(
                        "✓ Rate limit reset. Retrying...{}",
                        if pending.is_system {
                            " (system message)"
                        } else {
                            ""
                        }
                    )));
                    let _ = begin_remote_send(
                        app,
                        remote,
                        pending.content,
                        pending.images,
                        pending.is_system,
                    )
                    .await;
                    return;
                }
            }
        }
    }

    if !app.is_processing && !app.queued_messages.is_empty() {
        let messages = std::mem::take(&mut app.queued_messages);
        let combined = messages.join("\n\n");
        crate::logging::info(&format!(
            "Sending queued continuation message ({} chars)",
            combined.len()
        ));
        for msg in &messages {
            app.push_display_message(DisplayMessage::user(msg.clone()));
        }
        if begin_remote_send(app, remote, combined, vec![], true)
            .await
            .is_err()
        {
            crate::logging::error("Failed to send queued continuation message");
        }
    }

    detect_and_cancel_stall(app, remote).await;
}

pub(super) async fn handle_terminal_event(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    remote: &mut RemoteConnection,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<()> {
    match event {
        Some(Ok(Event::Key(key))) => {
            if key.kind == KeyEventKind::Press {
                handle_remote_key(app, key.code, key.modifiers, remote).await?;
                if let Some(spec) = app.pending_model_switch.take() {
                    let _ = remote.set_model(&spec).await;
                }
            }
        }
        Some(Ok(Event::Paste(text))) => {
            app.handle_paste(text);
        }
        Some(Ok(Event::Mouse(mouse))) => {
            handle_mouse_event(app, mouse);
        }
        Some(Ok(Event::Resize(_, _))) => {
            let _ = terminal.clear();
        }
        _ => {}
    }
    Ok(())
}

pub(super) async fn handle_bus_event(
    app: &mut App,
    remote: &mut RemoteConnection,
    bus_event: std::result::Result<BusEvent, tokio::sync::broadcast::error::RecvError>,
) {
    match bus_event {
        Ok(BusEvent::UsageReport(results)) => {
            app.handle_usage_report(results);
        }
        Ok(BusEvent::LoginCompleted(login)) => {
            let success = login.success && login.provider != "copilot_code";
            app.handle_login_completed(login);
            if success {
                let _ = remote.notify_auth_changed().await;
            }
        }
        Ok(BusEvent::UpdateStatus(status)) => {
            app.handle_update_status(status);
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

pub(super) async fn connect_with_retry(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event_stream: &mut EventStream,
    state: &mut RemoteRunState,
    session_to_resume: Option<&str>,
) -> Result<ConnectOutcome> {
    match RemoteConnection::connect_with_session(session_to_resume).await {
        Ok(remote) => {
            if let Some(idx) = state.disconnect_msg_idx.take() {
                if idx < app.display_messages.len() {
                    app.display_messages.remove(idx);
                }
            }
            state.disconnect_start = None;
            Ok(ConnectOutcome::Connected(remote))
        }
        Err(e) => {
            if state.reconnect_attempts == 0 && !app.server_spawning {
                return Err(anyhow::anyhow!(
                    "Failed to connect to server. Is `jcode serve` running? Error: {}",
                    e
                ));
            }

            let is_initial_server_start = app.server_spawning && state.reconnect_attempts == 0;
            if app.server_spawning && state.reconnect_attempts == 0 {
                state.reconnect_attempts = 1;
                app.server_spawning = false;
            }
            state.reconnect_attempts += 1;

            let elapsed = state
                .disconnect_start
                .get_or_insert_with(Instant::now)
                .elapsed();
            let elapsed_str = if elapsed.as_secs() < 60 {
                format!("{}s", elapsed.as_secs())
            } else {
                format!("{}m {}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
            };

            let session_name = app
                .remote_session_id
                .as_ref()
                .and_then(|id| crate::id::extract_session_name(id))
                .or_else(|| {
                    app.resume_session_id
                        .as_ref()
                        .and_then(|id| crate::id::extract_session_name(id))
                });
            let resume_hint = if let Some(name) = &session_name {
                format!("  Resume later: jcode --resume {}", name)
            } else {
                String::new()
            };

            let msg_content = if is_initial_server_start {
                "⏳ Starting server...".to_string()
            } else {
                format!(
                    "⚡ Connection lost — retrying ({})\n  {}\n{}",
                    elapsed_str, e, resume_hint,
                )
            };

            if let Some(idx) = state.disconnect_msg_idx {
                if idx < app.display_messages.len() {
                    app.display_messages[idx].content = msg_content;
                }
            } else {
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: msg_content,
                    tool_calls: Vec::new(),
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                state.disconnect_msg_idx = Some(app.display_messages.len() - 1);
            }
            terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;

            let backoff = if state.reconnect_attempts <= 2 {
                Duration::from_millis(100)
            } else {
                Duration::from_secs((1u64 << (state.reconnect_attempts - 2).min(5)).min(30))
            };
            let sleep = tokio::time::sleep(backoff);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    event = event_stream.next() => {
                        if let Some(Ok(Event::Key(key))) = event {
                            if key.kind == KeyEventKind::Press {
                                if key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    return Ok(ConnectOutcome::Quit);
                                }
                                if let Some(amount) = app
                                    .scroll_keys
                                    .scroll_amount(key.code.clone(), key.modifiers)
                                {
                                    if amount < 0 {
                                        app.scroll_up((-amount) as usize);
                                    } else {
                                        app.scroll_down(amount as usize);
                                    }
                                    terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
                                }
                            }
                        }
                    }
                }
            }

            Ok(ConnectOutcome::Retry)
        }
    }
}

pub(super) async fn handle_post_connect(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    remote: &mut RemoteConnection,
    state: &mut RemoteRunState,
    session_to_resume: Option<&str>,
) -> Result<PostConnectOutcome> {
    crate::logging::info(&format!(
        "Reload check: session_to_resume={:?}, remote_session_id={:?}, reconnect_attempts={}",
        session_to_resume, app.remote_session_id, state.reconnect_attempts
    ));
    let has_reload_ctx_for_session = session_to_resume
        .and_then(|sid| {
            let result = ReloadContext::peek_for_session(sid);
            crate::logging::info(&format!(
                "Reload peek_for_session({}) = {:?}",
                sid,
                result.as_ref().map(|r| r.is_some())
            ));
            result.ok().flatten()
        })
        .is_some();

    let has_client_reload_marker = session_to_resume
        .and_then(|sid| {
            let jcode_dir = crate::storage::jcode_dir().ok()?;
            let marker = jcode_dir.join(format!("client-reload-pending-{}", sid));
            if marker.exists() {
                let info = std::fs::read_to_string(&marker).ok()?;
                let _ = std::fs::remove_file(&marker);
                crate::logging::info(&format!(
                    "Found client-reload-pending marker for {}, injecting reload info",
                    sid
                ));
                if app.reload_info.is_empty() {
                    for line in info.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            app.reload_info.push(trimmed.to_string());
                        }
                    }
                }
                Some(())
            } else {
                None
            }
        })
        .is_some();

    if state.reconnect_attempts > 0 {
        if app.reload_info.is_empty() {
            if let Ok(jcode_dir) = crate::storage::jcode_dir() {
                let info_path = jcode_dir.join("reload-info");
                if info_path.exists() {
                    if let Ok(info) = std::fs::read_to_string(&info_path) {
                        let _ = std::fs::remove_file(&info_path);
                        let trimmed = info.trim();
                        if let Some(hash) = trimmed.strip_prefix("reload:") {
                            app.reload_info
                                .push(format!("Reloaded with build {}", hash.trim()));
                        } else if let Some(hash) = trimmed.strip_prefix("rebuild:") {
                            app.reload_info
                                .push(format!("Rebuilt and reloaded ({})", hash.trim()));
                        } else if !trimmed.is_empty() {
                            app.reload_info.push(trimmed.to_string());
                        }
                    }
                }
            }
        }

        if app.has_newer_binary() {
            app.push_display_message(DisplayMessage::system(
                "Server reloaded. Reloading client with newer binary...".to_string(),
            ));
            terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
            let session_id = app
                .remote_session_id
                .clone()
                .unwrap_or_else(|| crate::id::new_id("ses"));
            if has_reload_ctx_for_session || !app.reload_info.is_empty() {
                if let Ok(jcode_dir) = crate::storage::jcode_dir() {
                    let marker = jcode_dir.join(format!("client-reload-pending-{}", session_id));
                    let info = if app.reload_info.is_empty() {
                        "reload".to_string()
                    } else {
                        app.reload_info.join("\n")
                    };
                    let _ = std::fs::write(&marker, &info);
                    crate::logging::info(&format!(
                        "Wrote client-reload-pending marker for {} before re-exec",
                        session_id
                    ));
                }
            }
            app.save_input_for_reload(&session_id);
            app.reload_requested = Some(session_id);
            app.should_quit = true;
            return Ok(PostConnectOutcome::Quit);
        }

        let reload_details = if !app.reload_info.is_empty() {
            format!("\n  {}", app.reload_info.join("\n  "))
        } else if has_reload_ctx_for_session {
            "\n  Reload context restored".to_string()
        } else {
            String::new()
        };

        app.push_display_message(DisplayMessage::system(format!(
            "✓ Reconnected successfully.{}",
            reload_details
        )));
    }

    let should_queue_reload_continuation = has_reload_ctx_for_session || has_client_reload_marker;
    crate::logging::info(&format!(
        "Reload continuation check: should_queue={}, reload_info_empty={}, has_ctx={}, has_marker={}",
        should_queue_reload_continuation,
        app.reload_info.is_empty(),
        has_reload_ctx_for_session,
        has_client_reload_marker
    ));
    if should_queue_reload_continuation {
        let reload_ctx = session_to_resume.and_then(|sid| {
            let result = ReloadContext::load_for_session(sid);
            crate::logging::info(&format!(
                "Reload load_for_session({}) = {:?}",
                sid,
                result.as_ref().map(|r| r.is_some())
            ));
            result.ok().flatten()
        });

        if let Some(ctx) = reload_ctx {
            let task_info = ctx
                .task_context
                .map(|t| format!("\nTask context: {}", t))
                .unwrap_or_default();

            let continuation_msg = format!(
                "[SYSTEM: Reload succeeded. Build {} → {}.{}\nIMPORTANT: The reload is done. You MUST immediately continue your work. Do NOT ask the user what to do next. Do NOT summarize what happened. Just pick up exactly where you left off and keep going.]",
                ctx.version_before,
                ctx.version_after,
                task_info
            );

            crate::logging::info(&format!(
                "Queuing reload continuation message ({} chars)",
                continuation_msg.len()
            ));
            app.queued_messages.push(continuation_msg);
        } else {
            crate::logging::warn(
                "Reload context expected but not found (race condition?), skipping continuation",
            );
        }
        app.reload_info.clear();
    }

    state.reconnect_attempts = 0;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while !remote.has_loaded_history() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            crate::logging::warn("Timed out waiting for History event from server");
            break;
        }
        match tokio::time::timeout(remaining, remote.next_event()).await {
            Ok(Some(event)) => {
                handle_server_event(app, event, remote);
            }
            _ => break,
        }
    }

    Ok(PostConnectOutcome::Ready)
}

pub(super) async fn handle_remote_event(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    remote: &mut RemoteConnection,
    state: &mut RemoteRunState,
    event: Option<ServerEvent>,
) -> Result<RemoteEventOutcome> {
    match event {
        None => {
            app.rate_limit_pending_message = None;
            app.current_message_id = None;
            app.last_stream_activity = None;
            if let Some(chunk) = app.stream_buffer.flush() {
                app.streaming_text.push_str(&chunk);
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
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            app.streaming_tps_start = None;
            app.streaming_tps_elapsed = Duration::ZERO;
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            state.disconnect_start = Some(Instant::now());
            app.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "⚡ Connection lost — reconnecting…".to_string(),
                tool_calls: Vec::new(),
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            state.disconnect_msg_idx = Some(app.display_messages.len() - 1);
            terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
            state.reconnect_attempts = 1;
            Ok(RemoteEventOutcome::Reconnect)
        }
        Some(ServerEvent::ClientDebugRequest { id, command }) => {
            let output = handle_debug_command(app, &command, remote).await;
            let _ = remote.send_client_debug_response(id, output).await;
            process_remote_followups(app, remote).await;
            Ok(RemoteEventOutcome::Continue)
        }
        Some(server_event) => {
            let _ = handle_server_event(app, server_event, remote);
            process_remote_followups(app, remote).await;
            Ok(RemoteEventOutcome::Continue)
        }
    }
}

async fn process_remote_followups(app: &mut App, remote: &mut RemoteConnection) {
    if app.pending_server_reload && !app.is_processing {
        app.pending_server_reload = false;
        app.append_reload_message("Reloading server with newer binary...");
        let _ = remote.reload().await;
    }

    if app.is_processing {
        if let Some(interleave_msg) = app.interleave_message.take() {
            if !interleave_msg.trim().is_empty() {
                let msg_clone = interleave_msg.clone();
                if let Err(e) = remote.soft_interrupt(interleave_msg, false).await {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to queue soft interrupt: {}",
                        e
                    )));
                } else {
                    app.pending_soft_interrupts.push(msg_clone);
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
            if let Err(e) = begin_remote_send(app, remote, interleave_msg, vec![], false).await {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to send message: {}",
                    e
                )));
            }
        }
    } else if !app.queued_messages.is_empty() {
        let messages = std::mem::take(&mut app.queued_messages);
        let combined = messages.join("\n\n");
        for msg in &messages {
            app.push_display_message(DisplayMessage::user(msg.clone()));
        }
        let _ = begin_remote_send(app, remote, combined, vec![], true).await;
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
            crate::logging::warn(&format!(
                "Stream stall detected: no server events for {:?}, cancelling",
                app.last_stream_activity
                    .map(|t| t.elapsed())
                    .or(app.processing_started.map(|t| t.elapsed()))
            ));
            let _ = remote.cancel().await;
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            app.current_message_id = None;
            app.processing_started = None;
            app.last_stream_activity = None;
            app.rate_limit_pending_message = None;
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
            app.push_display_message(DisplayMessage::system(
                "⚠ Stream stalled (no response for 2 minutes). Processing cancelled. You can resend your message.".to_string(),
            ));
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

pub(super) async fn begin_remote_send(
    app: &mut App,
    remote: &mut RemoteConnection,
    content: String,
    images: Vec<(String, String)>,
    is_system: bool,
) -> Result<u64> {
    let msg_id = remote
        .send_message_with_images(content.clone(), images.clone())
        .await?;
    app.current_message_id = Some(msg_id);
    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.processing_started = Some(Instant::now());
    app.last_stream_activity = Some(Instant::now());
    app.streaming_tps_start = None;
    app.streaming_tps_elapsed = Duration::ZERO;
    app.streaming_total_output_tokens = 0;
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.rate_limit_pending_message = Some(super::PendingRemoteMessage {
        content,
        images,
        is_system,
    });
    remote.reset_call_output_tokens_seen();
    Ok(msg_id)
}

pub(super) fn handle_server_event(
    app: &mut App,
    event: ServerEvent,
    remote: &mut RemoteConnection,
) -> bool {
    if app.is_processing {
        app.last_stream_activity = Some(Instant::now());
    }

    let call_output_tokens_seen = remote.call_output_tokens_seen();

    match event {
        ServerEvent::TextDelta { text } => {
            if let Some(thought_line) = App::extract_thought_line(&text) {
                if let Some(chunk) = app.stream_buffer.flush() {
                    app.streaming_text.push_str(&chunk);
                }
                app.insert_thought_line(thought_line);
                return false;
            }
            if matches!(
                app.status,
                ProcessingStatus::Sending | ProcessingStatus::Connecting(_)
            ) {
                app.status = ProcessingStatus::Streaming;
            } else if matches!(app.status, ProcessingStatus::Thinking(_)) {
                app.status = ProcessingStatus::Streaming;
            } else if app.is_processing && matches!(app.status, ProcessingStatus::Idle) {
                app.status = ProcessingStatus::Streaming;
            }
            if app.streaming_tps_start.is_none() {
                app.streaming_tps_start = Some(Instant::now());
            }
            if let Some(chunk) = app.stream_buffer.push(&text) {
                app.streaming_text.push_str(&chunk);
            }
            app.last_stream_activity = Some(Instant::now());
            false
        }
        ServerEvent::TextReplace { text } => {
            app.stream_buffer.flush();
            app.streaming_text = text;
            false
        }
        ServerEvent::ToolStart { id, name } => {
            if app.streaming_tps_start.is_none() {
                app.streaming_tps_start = Some(Instant::now());
            }
            remote.handle_tool_start(&id, &name);
            if matches!(name.as_str(), "memory" | "remember") {
                crate::memory::set_state(crate::tui::info_widget::MemoryState::Embedding);
            }
            app.status = ProcessingStatus::RunningTool(name.clone());
            app.streaming_tool_calls.push(ToolCall {
                id,
                name,
                input: serde_json::Value::Null,
                intent: None,
            });
            false
        }
        ServerEvent::ToolInput { delta } => {
            remote.handle_tool_input(&delta);
            false
        }
        ServerEvent::ToolExec { id, name } => {
            if let Some(start) = app.streaming_tps_start.take() {
                app.streaming_tps_elapsed += start.elapsed();
            }
            let parsed_input = remote.get_current_tool_input();
            if let Some(tc) = app.streaming_tool_calls.iter_mut().find(|tc| tc.id == id) {
                tc.input = parsed_input.clone();
            }
            remote.handle_tool_exec(&id, &name);
            false
        }
        ServerEvent::ToolDone {
            id,
            name,
            output,
            error,
        } => {
            let _ = error;
            let display_output = remote.handle_tool_done(&id, &name, &output);
            let tool_input = app
                .streaming_tool_calls
                .iter()
                .find(|tc| tc.id == id)
                .map(|tc| tc.input.clone())
                .unwrap_or(serde_json::Value::Null);
            if let Some(chunk) = app.stream_buffer.flush() {
                app.streaming_text.push_str(&chunk);
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
            crate::tui::mermaid::clear_streaming_preview_diagram();
            app.push_display_message(DisplayMessage {
                role: "tool".to_string(),
                content: display_output,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: Some(ToolCall {
                    id,
                    name,
                    input: tool_input,
                    intent: None,
                }),
            });
            app.streaming_tool_calls.clear();
            app.status = ProcessingStatus::Streaming;
            true
        }
        ServerEvent::TokenUsage {
            input,
            output,
            cache_read_input,
            cache_creation_input,
        } => {
            app.accumulate_streaming_output_tokens(output, call_output_tokens_seen);
            app.streaming_input_tokens = input;
            app.streaming_output_tokens = output;
            if cache_read_input.is_some() {
                app.streaming_cache_read_tokens = cache_read_input;
            }
            if cache_creation_input.is_some() {
                app.streaming_cache_creation_tokens = cache_creation_input;
            }
            false
        }
        ServerEvent::ConnectionType { connection } => {
            app.connection_type = Some(connection);
            false
        }
        ServerEvent::ConnectionPhase { phase } => {
            let cp = match phase.as_str() {
                "authenticating" => crate::message::ConnectionPhase::Authenticating,
                "connecting" => crate::message::ConnectionPhase::Connecting,
                "waiting for response" => crate::message::ConnectionPhase::WaitingForResponse,
                "streaming" => crate::message::ConnectionPhase::Streaming,
                _ => crate::message::ConnectionPhase::Connecting,
            };
            app.status = ProcessingStatus::Connecting(cp);
            false
        }
        ServerEvent::UpstreamProvider { provider } => {
            app.upstream_provider = Some(provider);
            false
        }
        ServerEvent::Interrupted => {
            app.rate_limit_pending_message = None;
            app.interleave_message = None;
            app.pending_soft_interrupts.clear();
            if let Some(chunk) = app.stream_buffer.flush() {
                app.streaming_text.push_str(&chunk);
            }
            if !app.streaming_text.is_empty() {
                let content = app.take_streaming_text();
                app.push_display_message(DisplayMessage {
                    role: "assistant".to_string(),
                    content,
                    tool_calls: Vec::new(),
                    duration_secs: app.processing_started.map(|t| t.elapsed().as_secs_f32()),
                    title: None,
                    tool_data: None,
                });
            }
            app.clear_streaming_render_state();
            app.stream_buffer.clear();
            app.streaming_tool_calls.clear();
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            app.push_display_message(DisplayMessage::system("Interrupted"));
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            app.processing_started = None;
            app.current_message_id = None;
            remote.clear_pending();
            remote.reset_call_output_tokens_seen();
            false
        }
        ServerEvent::Done { id } => {
            crate::logging::info(&format!(
                "Client received Done id={}, current_message_id={:?}",
                id, app.current_message_id
            ));
            if app.current_message_id == Some(id) {
                app.rate_limit_pending_message = None;
                if let Some(chunk) = app.stream_buffer.flush() {
                    app.streaming_text.push_str(&chunk);
                }
                if let Some(start) = app.streaming_tps_start.take() {
                    app.streaming_tps_elapsed += start.elapsed();
                }
                if !app.streaming_text.is_empty() {
                    let duration = app.processing_started.map(|s| s.elapsed().as_secs_f32());
                    let content = app.take_streaming_text();
                    app.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content,
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    app.push_turn_footer(duration);
                }
                crate::tui::mermaid::clear_streaming_preview_diagram();
                app.rate_limit_pending_message = None;
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.processing_started = None;
                app.replay_processing_started_ms = None;
                app.replay_elapsed_override = None;
                app.streaming_tool_calls.clear();
                app.current_message_id = None;
                app.thought_line_inserted = false;
                app.thinking_prefix_emitted = false;
                app.thinking_buffer.clear();
                remote.clear_pending();
                remote.reset_call_output_tokens_seen();
            } else if app.is_processing {
                let is_stale = app.current_message_id.is_some_and(|mid| id < mid);
                if is_stale {
                    crate::logging::info(&format!(
                        "Ignoring stale Done id={} (current_message_id={:?}), likely from Subscribe/ResumeSession",
                        id, app.current_message_id
                    ));
                } else {
                    crate::logging::warn(&format!(
                        "Done id={} doesn't match current_message_id={:?} but is_processing=true, forcing idle",
                        id, app.current_message_id
                    ));
                    if let Some(chunk) = app.stream_buffer.flush() {
                        app.streaming_text.push_str(&chunk);
                    }
                    if !app.streaming_text.is_empty() {
                        let duration = app.processing_started.map(|s| s.elapsed().as_secs_f32());
                        let content = app.take_streaming_text();
                        app.push_display_message(DisplayMessage {
                            role: "assistant".to_string(),
                            content,
                            tool_calls: vec![],
                            duration_secs: duration,
                            title: None,
                            tool_data: None,
                        });
                        app.push_turn_footer(duration);
                    }
                    crate::tui::mermaid::clear_streaming_preview_diagram();
                    app.is_processing = false;
                    app.status = ProcessingStatus::Idle;
                    app.processing_started = None;
                    app.replay_processing_started_ms = None;
                    app.replay_elapsed_override = None;
                    app.streaming_tool_calls.clear();
                    app.current_message_id = None;
                    app.thought_line_inserted = false;
                    app.thinking_prefix_emitted = false;
                    app.thinking_buffer.clear();
                    remote.clear_pending();
                    remote.reset_call_output_tokens_seen();
                }
            }
            false
        }
        ServerEvent::Error {
            message,
            retry_after_secs,
            ..
        } => {
            let reset_duration = retry_after_secs
                .map(Duration::from_secs)
                .or_else(|| parse_rate_limit_error(&message));
            if let Some(reset_duration) = reset_duration {
                app.rate_limit_reset = Some(Instant::now() + reset_duration);
                if let Some(is_system) = app
                    .rate_limit_pending_message
                    .as_ref()
                    .map(|pending| pending.is_system)
                {
                    app.push_display_message(DisplayMessage::system(format!(
                        "⏳ Rate limit hit. Will auto-retry in {} seconds...",
                        reset_duration.as_secs()
                    )));
                    if is_system {
                        app.set_status_notice("Rate limited; queued system retry");
                    } else {
                        app.set_status_notice("Rate limited; queued retry");
                    }
                    app.is_processing = false;
                    app.status = ProcessingStatus::Idle;
                    app.processing_started = None;
                    app.current_message_id = None;
                    remote.clear_pending();
                    remote.reset_call_output_tokens_seen();
                    return false;
                }
            }
            app.push_display_message(DisplayMessage {
                role: "error".to_string(),
                content: message,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            app.rate_limit_pending_message = None;
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            app.interleave_message = None;
            app.pending_soft_interrupts.clear();
            crate::tui::mermaid::clear_streaming_preview_diagram();
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            remote.clear_pending();
            remote.reset_call_output_tokens_seen();
            false
        }
        ServerEvent::SessionId { session_id } => {
            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            app.update_terminal_title();
            false
        }
        ServerEvent::Reloading { .. } => {
            app.append_reload_message("🔄 Server reload initiated...");
            false
        }
        ServerEvent::ReloadProgress {
            step,
            message,
            success,
            output,
        } => {
            let mut content = if let Some(ok) = success {
                let status_icon = if ok { "✓" } else { "✗" };
                format!("[{}] {} {}", step, status_icon, message)
            } else {
                format!("[{}] {}", step, message)
            };

            if let Some(out) = output {
                if !out.is_empty() {
                    content.push_str("\n```\n");
                    content.push_str(&out);
                    content.push_str("\n```");
                }
            }

            app.append_reload_message(&content);

            if step == "verify" || step == "git" {
                app.reload_info.push(message.clone());
            }

            app.status_notice = Some((format!("Reload: {}", message), std::time::Instant::now()));
            false
        }
        ServerEvent::History {
            messages,
            session_id,
            provider_name,
            provider_model,
            available_models,
            available_model_routes,
            mcp_servers,
            skills: _skills,
            all_sessions,
            client_count,
            is_canary,
            server_version,
            server_name,
            server_icon,
            server_has_update,
            was_interrupted,
            upstream_provider,
            ..
        } => {
            let prev_session_id = app.remote_session_id.clone();
            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            let session_changed = prev_session_id.as_deref() != Some(session_id.as_str());

            if session_changed {
                app.rate_limit_pending_message = None;
                app.rate_limit_reset = None;
                app.clear_display_messages();
                app.clear_streaming_render_state();
                app.streaming_tool_calls.clear();
                app.thought_line_inserted = false;
                app.thinking_prefix_emitted = false;
                app.thinking_buffer.clear();
                app.streaming_input_tokens = 0;
                app.streaming_output_tokens = 0;
                app.streaming_cache_read_tokens = None;
                app.streaming_cache_creation_tokens = None;
                app.processing_started = None;
                app.replay_processing_started_ms = None;
                app.replay_elapsed_override = None;
                app.streaming_tps_start = None;
                app.streaming_tps_elapsed = Duration::ZERO;
                app.streaming_total_output_tokens = 0;
                app.last_stream_activity = None;
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.follow_chat_bottom();
                if prev_session_id.is_some() {
                    app.queued_messages.clear();
                }
                app.interleave_message = None;
                app.pending_soft_interrupts.clear();
                app.remote_total_tokens = None;
                app.remote_swarm_members.clear();
                app.swarm_plan_items.clear();
                app.swarm_plan_version = None;
                app.swarm_plan_swarm_id = None;
                app.connection_type = None;
                remote.reset_call_output_tokens_seen();
            }
            if let Some(name) = provider_name {
                app.remote_provider_name = Some(name);
            }
            if let Some(model) = provider_model {
                app.update_context_limit_for_model(&model);
                app.remote_provider_model = Some(model);
            }
            if upstream_provider.is_some() {
                app.upstream_provider = upstream_provider;
            }
            app.remote_available_models = available_models;
            app.remote_model_routes = available_model_routes;
            app.remote_sessions = all_sessions;
            app.remote_client_count = client_count;
            app.remote_is_canary = is_canary;
            app.remote_server_version = server_version;
            app.remote_server_has_update = server_has_update;

            if server_has_update == Some(true) && !app.pending_server_reload {
                app.pending_server_reload = true;
                app.set_status_notice("Server update available, reloading...");
            }
            app.remote_server_short_name = server_name;
            if let Some(icon) = server_icon {
                app.remote_server_icon = Some(icon);
            }

            app.update_terminal_title();

            if !mcp_servers.is_empty() {
                app.mcp_server_names = mcp_servers
                    .iter()
                    .filter_map(|s| {
                        let (name, count_str) = s.split_once(':')?;
                        let count = count_str.parse::<usize>().unwrap_or(0);
                        Some((name.to_string(), count))
                    })
                    .collect();
            }

            if session_changed || !remote.has_loaded_history() {
                remote.mark_history_loaded();
                for msg in messages {
                    app.push_display_message(DisplayMessage {
                        role: msg.role,
                        content: msg.content,
                        tool_calls: msg.tool_calls.unwrap_or_default(),
                        duration_secs: None,
                        title: None,
                        tool_data: msg.tool_data,
                    });
                }
            }

            if was_interrupted == Some(true) && !app.display_messages.is_empty() {
                crate::logging::info(
                    "Session was interrupted mid-generation, queuing continuation",
                );
                app.push_display_message(DisplayMessage::system(
                    "⚡ Session was interrupted mid-generation. Continuing...".to_string(),
                ));
                app.queued_messages.push(
                    "[SYSTEM: Your session was interrupted by a server reload while you were working. \
                     The session has been restored. Any tool that was running was aborted and its results \
                     may be incomplete. Please continue exactly where you left off. \
                     Look at the conversation history to understand what you were doing and resume immediately. \
                     Do NOT ask the user what to do - just continue your work.]"
                        .to_string(),
                );
            }

            false
        }
        ServerEvent::SwarmStatus { members } => {
            if app.swarm_enabled {
                app.remote_swarm_members = members;
            } else {
                app.remote_swarm_members.clear();
            }
            false
        }
        ServerEvent::SwarmPlan {
            swarm_id,
            version,
            items,
            ..
        } => {
            app.swarm_plan_swarm_id = Some(swarm_id);
            app.swarm_plan_version = Some(version);
            app.swarm_plan_items = items;
            app.set_status_notice(format!(
                "Swarm plan synced (v{}, {} items)",
                version,
                app.swarm_plan_items.len()
            ));
            false
        }
        ServerEvent::SwarmPlanProposal {
            swarm_id,
            proposer_session,
            proposer_name,
            summary,
            ..
        } => {
            let proposer =
                proposer_name.unwrap_or_else(|| proposer_session.chars().take(8).collect());
            app.push_display_message(DisplayMessage::system(format!(
                "Plan proposal received in swarm {}\nFrom: {}\nSummary: {}",
                swarm_id, proposer, summary
            )));
            app.set_status_notice("Plan proposal received");
            false
        }
        ServerEvent::McpStatus { servers } => {
            app.mcp_server_names = servers
                .iter()
                .filter_map(|s| {
                    let (name, count_str) = s.split_once(':')?;
                    let count = count_str.parse::<usize>().unwrap_or(0);
                    Some((name.to_string(), count))
                })
                .collect();
            false
        }
        ServerEvent::ModelChanged {
            model,
            provider_name,
            error,
            ..
        } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch model: {}",
                    err
                )));
                app.set_status_notice("Model switch failed");
            } else {
                app.update_context_limit_for_model(&model);
                app.remote_provider_model = Some(model.clone());
                if let Some(ref pname) = provider_name {
                    app.remote_provider_name = Some(pname.clone());
                }
                app.connection_type = None;
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Switched to model: {}",
                    model
                )));
                app.set_status_notice(format!("Model → {}", model));
            }
            false
        }
        ServerEvent::AvailableModelsUpdated {
            available_models,
            available_model_routes,
        } => {
            app.remote_available_models = available_models;
            app.remote_model_routes = available_model_routes;
            false
        }
        ServerEvent::SoftInterruptInjected {
            content,
            point: _,
            tools_skipped,
        } => {
            if let Some(chunk) = app.stream_buffer.flush() {
                app.streaming_text.push_str(&chunk);
            }
            if !app.streaming_text.is_empty() {
                let duration = app.processing_started.map(|s| s.elapsed().as_secs_f32());
                let flushed = app.take_streaming_text();
                app.push_display_message(DisplayMessage {
                    role: "assistant".to_string(),
                    content: flushed,
                    tool_calls: vec![],
                    duration_secs: duration,
                    title: None,
                    tool_data: None,
                });
                app.push_turn_footer(duration);
            }
            app.pending_soft_interrupts.clear();
            app.push_display_message(DisplayMessage {
                role: "user".to_string(),
                content: content.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            if let Some(n) = tools_skipped {
                app.set_status_notice(format!("⚡ {} tool(s) skipped", n));
            }
            false
        }
        ServerEvent::MemoryInjected {
            count,
            prompt,
            prompt_chars: _,
            computed_age_ms,
        } => {
            if app.memory_enabled {
                let plural = if count == 1 { "memory" } else { "memories" };
                let display_prompt = if prompt.trim().is_empty() {
                    "# Memory\n\n## Notes\n1. (content unavailable from server event)".to_string()
                } else {
                    prompt.clone()
                };
                crate::memory::record_injected_prompt(&display_prompt, count, computed_age_ms);
                let summary = if count == 1 {
                    "🧠 auto-recalled 1 memory".to_string()
                } else {
                    format!("🧠 auto-recalled {} memories", count)
                };
                app.push_display_message(DisplayMessage::memory(summary, display_prompt));
                app.set_status_notice(format!("🧠 {} relevant {} injected", count, plural));
            }
            false
        }
        ServerEvent::SplitResponse {
            new_session_id,
            new_session_name,
            ..
        } => {
            let exe = std::env::current_exe().unwrap_or_default();
            let cwd = std::env::current_dir().unwrap_or_default();
            let socket = std::env::var("JCODE_SOCKET").ok();
            match spawn_in_new_terminal(&exe, &new_session_id, &cwd, socket.as_deref()) {
                Ok(true) => {
                    app.push_display_message(DisplayMessage::system(format!(
                        "✂ Split → **{}** (opened in new window)",
                        new_session_name,
                    )));
                    app.set_status_notice(format!("Split → {}", new_session_name));
                }
                Ok(false) => {
                    app.push_display_message(DisplayMessage::system(format!(
                        "✂ Split → **{}**\n\nNo terminal found. Resume manually:\n```\njcode --resume {}\n```",
                        new_session_name, new_session_id,
                    )));
                }
                Err(e) => {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Split created **{}** but failed to open window: {}\n\nResume manually: `jcode --resume {}`",
                        new_session_name, e, new_session_id,
                    )));
                }
            }
            false
        }
        ServerEvent::CompactResult {
            message, success, ..
        } => {
            if success {
                app.push_display_message(DisplayMessage::system(message));
                app.set_status_notice("📦 Compaction started");
            } else {
                app.push_display_message(DisplayMessage::system(message));
                app.set_status_notice("Compaction failed");
            }
            false
        }
        ServerEvent::StdinRequest { .. } => {
            app.set_status_notice("⌨ Interactive terminal detected (command will timeout)");
            false
        }
        _ => false,
    }
}

pub(super) fn handle_remote_char_input(app: &mut App, c: char) {
    if app.input.is_empty() && !app.is_processing && app.display_messages.is_empty() {
        if let Some(digit) = c.to_digit(10) {
            let suggestions = app.suggestion_prompts();
            let idx = digit as usize;
            if idx >= 1 && idx <= suggestions.len() {
                let (_label, prompt) = &suggestions[idx - 1];
                if !prompt.starts_with('/') {
                    app.input = prompt.clone();
                    app.cursor_pos = app.input.len();
                    app.follow_chat_bottom();
                    return;
                }
            }
        }
    }
    app.input.insert(app.cursor_pos, c);
    app.cursor_pos += c.len_utf8();
    app.follow_chat_bottom();
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
}

pub(super) async fn send_interleave_now(
    app: &mut App,
    content: String,
    remote: &mut RemoteConnection,
) {
    if content.trim().is_empty() {
        return;
    }
    let msg_clone = content.clone();
    if let Err(e) = remote.soft_interrupt(content, false).await {
        app.push_display_message(DisplayMessage::error(format!(
            "Failed to send interleave: {}",
            e
        )));
    } else {
        app.pending_soft_interrupts.push(msg_clone);
        app.set_status_notice("⏭ Interleave sent");
    }
}

pub(super) async fn handle_remote_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    remote: &mut RemoteConnection,
) -> Result<()> {
    let mut code = code;
    let mut modifiers = modifiers;
    ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

    if app.changelog_scroll.is_some() {
        return app.handle_changelog_key(code);
    }

    if app.help_scroll.is_some() {
        return app.handle_help_key(code);
    }

    if app.session_picker_overlay.is_some() {
        return app.handle_session_picker_key(code, modifiers);
    }

    if let Some(ref picker) = app.picker_state {
        if !picker.preview {
            return app.handle_picker_key(code, modifiers);
        }
    }

    if app.handle_picker_preview_key(&code, modifiers)? {
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('m')) {
        app.toggle_diagram_pane();
        return Ok(());
    }
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('t')) {
        app.toggle_diagram_pane_position();
        return Ok(());
    }
    if let Some(direction) = app.model_switch_keys.direction_for(code.clone(), modifiers) {
        remote.cycle_model(direction).await?;
        return Ok(());
    }
    if let Some(direction) = app
        .effort_switch_keys
        .direction_for(code.clone(), modifiers)
    {
        app.cycle_effort(direction);
        return Ok(());
    }
    if app
        .centered_toggle_keys
        .toggle
        .matches(code.clone(), modifiers)
    {
        app.toggle_centered_mode();
        return Ok(());
    }
    app.normalize_diagram_state();
    let diagram_available = app.diagram_available();
    if app.handle_diagram_focus_key(code.clone(), modifiers, diagram_available) {
        return Ok(());
    }
    if app.handle_diff_pane_focus_key(code.clone(), modifiers) {
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::ALT) {
        match code {
            KeyCode::Char('b') => {
                if matches!(app.status, ProcessingStatus::RunningTool(_)) {
                    remote.background_tool().await?;
                    app.set_status_notice("Moving tool to background...");
                    return Ok(());
                }
                app.cursor_pos = app.find_word_boundary_back();
                return Ok(());
            }
            KeyCode::Char('f') => {
                app.cursor_pos = app.find_word_boundary_forward();
                return Ok(());
            }
            KeyCode::Char('d') => {
                let end = app.find_word_boundary_forward();
                app.input.drain(app.cursor_pos..end);
                return Ok(());
            }
            KeyCode::Backspace => {
                let start = app.find_word_boundary_back();
                app.input.drain(start..app.cursor_pos);
                app.cursor_pos = start;
                return Ok(());
            }
            KeyCode::Char('v') => {
                app.paste_image_from_clipboard();
                return Ok(());
            }
            _ => {}
        }
    }

    if let Some(amount) = app.scroll_keys.scroll_amount(code.clone(), modifiers) {
        if amount < 0 {
            app.scroll_up((-amount) as usize);
        } else {
            app.scroll_down(amount as usize);
        }
        return Ok(());
    }

    if let Some(dir) = app.scroll_keys.prompt_jump(code.clone(), modifiers) {
        if dir < 0 {
            app.scroll_to_prev_prompt();
        } else {
            app.scroll_to_next_prompt();
        }
        return Ok(());
    }

    if let Some(rank) = App::ctrl_prompt_rank(&code, modifiers) {
        app.scroll_to_recent_prompt_rank(rank);
        return Ok(());
    }

    if app
        .centered_toggle_keys
        .toggle
        .matches(code.clone(), modifiers)
    {
        app.toggle_centered_mode();
        return Ok(());
    }

    if app.scroll_keys.is_bookmark(code.clone(), modifiers) {
        app.toggle_scroll_bookmark();
        return Ok(());
    }

    if code == KeyCode::BackTab {
        app.diff_mode = app.diff_mode.cycle();
        if !app.diff_mode.has_side_pane() {
            app.diff_pane_focus = false;
        }
        let status = format!("Diffs: {}", app.diff_mode.label());
        app.set_status_notice(&status);
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        if app.handle_diagram_ctrl_key(code.clone(), diagram_available) {
            return Ok(());
        }
        match code {
            KeyCode::Char('b') => {
                if matches!(app.status, ProcessingStatus::RunningTool(_)) {
                    remote.background_tool().await?;
                    app.set_status_notice("Moving tool to background...");
                    return Ok(());
                }
            }
            KeyCode::Char('c') | KeyCode::Char('d') => {
                app.handle_quit_request();
                return Ok(());
            }
            KeyCode::Char('r') => {
                app.recover_session_without_tools();
                return Ok(());
            }
            KeyCode::Char('l')
                if !app.is_processing && !diagram_available && !app.diff_pane_visible() =>
            {
                app.clear_display_messages();
                app.queued_messages.clear();
                return Ok(());
            }
            KeyCode::Char('u') => {
                app.input.drain(..app.cursor_pos);
                app.cursor_pos = 0;
                return Ok(());
            }
            KeyCode::Char('k') => {
                app.input.truncate(app.cursor_pos);
                return Ok(());
            }
            KeyCode::Char('a') => {
                app.cursor_pos = 0;
                return Ok(());
            }
            KeyCode::Char('e') => {
                app.cursor_pos = app.input.len();
                return Ok(());
            }
            KeyCode::Char('w') => {
                let start = app.find_word_boundary_back();
                app.input.drain(start..app.cursor_pos);
                app.cursor_pos = start;
                return Ok(());
            }
            KeyCode::Char('s') => {
                app.toggle_input_stash();
                return Ok(());
            }
            KeyCode::Char('v') => {
                app.paste_image_from_clipboard();
                return Ok(());
            }
            KeyCode::Tab | KeyCode::Char('t') => {
                app.queue_mode = !app.queue_mode;
                let mode_str = if app.queue_mode {
                    "Queue mode: messages wait until response completes"
                } else {
                    "Immediate mode: messages send next (no interrupt)"
                };
                app.set_status_notice(mode_str);
                return Ok(());
            }
            KeyCode::Up => {
                let had_pending = app.retrieve_pending_message_for_edit();
                if had_pending {
                    let _ = remote.cancel_soft_interrupts().await;
                }
                return Ok(());
            }
            _ => {}
        }
    }

    if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
        if !app.input.is_empty() {
            let raw_input = std::mem::take(&mut app.input);
            let expanded = app.expand_paste_placeholders(&raw_input);
            app.pasted_contents.clear();
            let images = std::mem::take(&mut app.pending_images);
            app.cursor_pos = 0;

            match app.send_action(true) {
                SendAction::Submit => {
                    app.push_display_message(DisplayMessage {
                        role: "user".to_string(),
                        content: raw_input,
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    let _ = app.begin_remote_send(remote, expanded, images, false).await;
                }
                SendAction::Queue => {
                    app.queued_messages.push(expanded);
                }
                SendAction::Interleave => {
                    app.send_interleave_now(expanded, remote).await;
                }
            }
        }
        return Ok(());
    }

    if app
        .picker_state
        .as_ref()
        .map(|p| p.preview)
        .unwrap_or(false)
    {
        match code {
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                return app.handle_picker_key(code, modifiers);
            }
            _ => {}
        }
    }

    match code {
        KeyCode::Char(c) => {
            handle_remote_char_input(app, c);
        }
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                let prev = super::super::core::prev_char_boundary(&app.input, app.cursor_pos);
                app.input.drain(prev..app.cursor_pos);
                app.cursor_pos = prev;
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.input.len() {
                let next = super::super::core::next_char_boundary(&app.input, app.cursor_pos);
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
        KeyCode::Home => {
            app.cursor_pos = 0;
        }
        KeyCode::End => {
            app.cursor_pos = app.input.len();
        }
        KeyCode::Tab => {
            app.autocomplete();
        }
        KeyCode::Enter => {
            if app.activate_model_picker_from_preview() {
                return Ok(());
            }
            if !app.input.is_empty() {
                let raw_input = std::mem::take(&mut app.input);
                let expanded = app.expand_paste_placeholders(&raw_input);
                app.pasted_contents.clear();
                let images = std::mem::take(&mut app.pending_images);
                app.cursor_pos = 0;
                let trimmed = expanded.trim();

                if let Some(topic) = trimmed
                    .strip_prefix("/help ")
                    .or_else(|| trimmed.strip_prefix("/? "))
                {
                    if let Some(help) = app.command_help(topic) {
                        app.push_display_message(DisplayMessage::system(help));
                    } else {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Unknown command '{}'. Use `/help` to list commands.",
                            topic.trim()
                        )));
                    }
                    return Ok(());
                }

                if trimmed == "/help" || trimmed == "/?" || trimmed == "/commands" {
                    app.help_scroll = Some(0);
                    return Ok(());
                }

                if trimmed == "/reload" {
                    let client_needs_reload = app.has_newer_binary();
                    let server_needs_reload =
                        app.remote_server_has_update.unwrap_or(client_needs_reload);

                    if !client_needs_reload && !server_needs_reload {
                        app.push_display_message(DisplayMessage::system(
                            "No newer binary found. Nothing to reload.".to_string(),
                        ));
                        return Ok(());
                    }

                    if server_needs_reload {
                        app.append_reload_message("Reloading server with newer binary...");
                        remote.reload().await?;
                    }

                    if client_needs_reload {
                        app.push_display_message(DisplayMessage::system(
                            "Reloading client with newer binary...".to_string(),
                        ));
                        let session_id = app
                            .remote_session_id
                            .clone()
                            .unwrap_or_else(|| crate::id::new_id("ses"));
                        app.save_input_for_reload(&session_id);
                        app.reload_requested = Some(session_id);
                        app.should_quit = true;
                    }
                    return Ok(());
                }

                if trimmed == "/client-reload" {
                    app.push_display_message(DisplayMessage::system(
                        "Reloading client...".to_string(),
                    ));
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
                    app.save_input_for_reload(&session_id);
                    app.reload_requested = Some(session_id);
                    app.should_quit = true;
                    return Ok(());
                }

                if trimmed == "/server-reload" {
                    app.append_reload_message("Reloading server...");
                    remote.reload().await?;
                    return Ok(());
                }

                if trimmed == "/rebuild" {
                    app.push_display_message(DisplayMessage::system(
                        "Rebuilding (git pull + cargo build + tests)...".to_string(),
                    ));
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
                    app.rebuild_requested = Some(session_id);
                    app.should_quit = true;
                    return Ok(());
                }

                if trimmed == "/update" {
                    app.push_display_message(DisplayMessage::system(
                        "Checking for updates...".to_string(),
                    ));
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
                    app.update_requested = Some(session_id);
                    app.should_quit = true;
                    return Ok(());
                }

                if trimmed == "/quit" {
                    app.session.mark_closed();
                    let _ = app.session.save();
                    app.should_quit = true;
                    return Ok(());
                }

                if trimmed == "/model" || trimmed == "/models" {
                    app.open_model_picker();
                    return Ok(());
                }

                if let Some(model_name) = trimmed.strip_prefix("/model ") {
                    let model_name = model_name.trim();
                    if model_name.is_empty() {
                        app.push_display_message(DisplayMessage::error("Usage: /model <name>"));
                        return Ok(());
                    }
                    app.upstream_provider = None;
                    app.connection_type = None;
                    remote.set_model(model_name).await?;
                    return Ok(());
                }

                if trimmed == "/account" || trimmed == "/accounts" {
                    app.input = trimmed.to_string();
                    app.cursor_pos = app.input.len();
                    app.submit_input();
                    return Ok(());
                }

                if let Some(sub) = trimmed.strip_prefix("/account ") {
                    let parts: Vec<&str> = sub.trim().splitn(2, ' ').collect();
                    if matches!(parts[0], "switch" | "use") {
                        if let Some(label) =
                            parts.get(1).map(|s| s.trim()).filter(|s| !s.is_empty())
                        {
                            if let Err(e) = crate::auth::claude::set_active_account(label) {
                                app.push_display_message(DisplayMessage::error(format!(
                                    "Failed to switch account: {}",
                                    e
                                )));
                                return Ok(());
                            }
                            crate::auth::AuthStatus::invalidate_cache();
                            app.context_limit = app.provider.context_window() as u64;
                            app.context_warning_shown = false;
                            remote.switch_anthropic_account(label).await?;
                            app.push_display_message(DisplayMessage::system(format!(
                                "Switched to Anthropic account `{}`.",
                                label
                            )));
                            app.set_status_notice(&format!("Account: switched to {}", label));
                            return Ok(());
                        }
                        app.push_display_message(DisplayMessage::error(
                            "Usage: `/account switch <label>`".to_string(),
                        ));
                        return Ok(());
                    }

                    app.input = trimmed.to_string();
                    app.cursor_pos = app.input.len();
                    app.submit_input();
                    return Ok(());
                }

                if trimmed == "/memory status" {
                    let default_enabled = crate::config::config().features.memory;
                    app.push_display_message(DisplayMessage::system(format!(
                        "Memory feature: **{}** (config default: {})",
                        if app.memory_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        if default_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    )));
                    return Ok(());
                }

                if trimmed == "/memory" {
                    let new_state = !app.memory_enabled;
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Memory, new_state)
                        .await?;
                    app.set_memory_feature_enabled(new_state);
                    let label = if new_state { "ON" } else { "OFF" };
                    app.set_status_notice(&format!("Memory: {}", label));
                    app.push_display_message(DisplayMessage::system(format!(
                        "Memory feature {} for this session.",
                        if new_state { "enabled" } else { "disabled" }
                    )));
                    return Ok(());
                }

                if trimmed == "/memory on" {
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Memory, true)
                        .await?;
                    app.set_memory_feature_enabled(true);
                    app.set_status_notice("Memory: ON");
                    app.push_display_message(DisplayMessage::system(
                        "Memory feature enabled for this session.".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed == "/memory off" {
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Memory, false)
                        .await?;
                    app.set_memory_feature_enabled(false);
                    app.set_status_notice("Memory: OFF");
                    app.push_display_message(DisplayMessage::system(
                        "Memory feature disabled for this session.".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed.starts_with("/memory ") {
                    app.push_display_message(DisplayMessage::error(
                        "Usage: /memory [on|off|status]".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed == "/swarm" || trimmed == "/swarm status" {
                    let default_enabled = crate::config::config().features.swarm;
                    app.push_display_message(DisplayMessage::system(format!(
                        "Swarm feature: **{}** (config default: {})",
                        if app.swarm_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        if default_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    )));
                    return Ok(());
                }

                if trimmed == "/swarm on" {
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Swarm, true)
                        .await?;
                    app.set_swarm_feature_enabled(true);
                    app.set_status_notice("Swarm: ON");
                    app.push_display_message(DisplayMessage::system(
                        "Swarm feature enabled for this session.".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed == "/swarm off" {
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Swarm, false)
                        .await?;
                    app.set_swarm_feature_enabled(false);
                    app.set_status_notice("Swarm: OFF");
                    app.push_display_message(DisplayMessage::system(
                        "Swarm feature disabled for this session.".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed.starts_with("/swarm ") {
                    app.push_display_message(DisplayMessage::error(
                        "Usage: /swarm [on|off|status]".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed == "/resume" || trimmed == "/sessions" {
                    app.open_session_picker();
                    return Ok(());
                }

                if trimmed == "/save" || trimmed.starts_with("/save ") {
                    let label = trimmed.strip_prefix("/save").unwrap().trim();
                    let label = if label.is_empty() {
                        None
                    } else {
                        Some(label.to_string())
                    };
                    app.session.mark_saved(label.clone());
                    if let Err(e) = app.session.save() {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to save session: {}",
                            e
                        )));
                        return Ok(());
                    }
                    let name = app.session.display_name().to_string();
                    let msg = if let Some(ref lbl) = app.session.save_label {
                        format!(
                            "📌 Session **{}** saved as \"**{}**\". It will appear at the top of `/resume`.",
                            name, lbl,
                        )
                    } else {
                        format!(
                            "📌 Session **{}** saved. It will appear at the top of `/resume`.",
                            name,
                        )
                    };
                    app.push_display_message(DisplayMessage::system(msg));
                    app.set_status_notice("Session saved");
                    return Ok(());
                }

                if trimmed == "/unsave" {
                    app.session.unmark_saved();
                    if let Err(e) = app.session.save() {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to save session: {}",
                            e
                        )));
                        return Ok(());
                    }
                    let name = app.session.display_name().to_string();
                    app.push_display_message(DisplayMessage::system(format!(
                        "Removed bookmark from session **{}**.",
                        name,
                    )));
                    app.set_status_notice("Bookmark removed");
                    return Ok(());
                }

                if trimmed == "/split" {
                    if app.is_processing {
                        app.push_display_message(DisplayMessage::error(
                            "Cannot split while processing. Wait for the current turn to finish."
                                .to_string(),
                        ));
                        return Ok(());
                    }
                    app.push_display_message(DisplayMessage::system(
                        "Splitting session...".to_string(),
                    ));
                    remote.split().await?;
                    return Ok(());
                }

                if trimmed == "/compact" {
                    app.push_display_message(DisplayMessage::system(
                        "Requesting compaction...".to_string(),
                    ));
                    remote.compact().await?;
                    return Ok(());
                }

                if app.pending_login.is_some() {
                    app.input = trimmed.to_string();
                    app.cursor_pos = app.input.len();
                    app.submit_input();
                    return Ok(());
                }

                if trimmed == "/z" || trimmed == "/zz" || trimmed == "/zzz" {
                    use crate::provider::copilot::PremiumMode;
                    let current = app.provider.premium_mode();

                    if trimmed == "/z" {
                        app.provider.set_premium_mode(PremiumMode::Normal);
                        let _ = remote.set_premium_mode(PremiumMode::Normal as u8).await;
                        let _ = crate::config::Config::set_copilot_premium(None);
                        app.set_status_notice("Premium: normal");
                        app.push_display_message(DisplayMessage::system(
                            "Premium request mode reset to normal. (saved to config)".to_string(),
                        ));
                        return Ok(());
                    }

                    let mode = if trimmed == "/zzz" {
                        PremiumMode::Zero
                    } else {
                        PremiumMode::OnePerSession
                    };
                    if current == mode {
                        app.provider.set_premium_mode(PremiumMode::Normal);
                        let _ = remote.set_premium_mode(PremiumMode::Normal as u8).await;
                        let _ = crate::config::Config::set_copilot_premium(None);
                        app.set_status_notice("Premium: normal");
                        app.push_display_message(DisplayMessage::system(
                            "Premium request mode reset to normal. (saved to config)".to_string(),
                        ));
                    } else {
                        app.provider.set_premium_mode(mode);
                        let _ = remote.set_premium_mode(mode as u8).await;
                        let config_val = match mode {
                            PremiumMode::Zero => "zero",
                            PremiumMode::OnePerSession => "one",
                            PremiumMode::Normal => "normal",
                        };
                        let _ = crate::config::Config::set_copilot_premium(Some(config_val));
                        let label = match mode {
                            PremiumMode::OnePerSession => "one premium per session",
                            PremiumMode::Zero => "zero premium requests",
                            PremiumMode::Normal => "normal",
                        };
                        app.set_status_notice(&format!("Premium: {}", label));
                        app.push_display_message(DisplayMessage::system(format!(
                            "Premium mode: **{}**. Toggle off with `/z`. (saved to config)",
                            label,
                        )));
                    }
                    return Ok(());
                }

                if trimmed.starts_with('/') {
                    app.input = trimmed.to_string();
                    app.cursor_pos = app.input.len();
                    app.submit_input();
                    return Ok(());
                }

                match app.send_action(false) {
                    SendAction::Submit => {
                        app.push_display_message(DisplayMessage {
                            role: "user".to_string(),
                            content: raw_input,
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                        let _ = app.begin_remote_send(remote, expanded, images, false).await;
                    }
                    SendAction::Queue => {
                        app.queued_messages.push(expanded);
                    }
                    SendAction::Interleave => {
                        app.send_interleave_now(expanded, remote).await;
                    }
                }
            }
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
            if app
                .picker_state
                .as_ref()
                .map(|p| p.preview)
                .unwrap_or(false)
            {
                app.picker_state = None;
                app.input.clear();
                app.cursor_pos = 0;
            } else if app.is_processing {
                remote.cancel().await?;
                app.set_status_notice("Interrupting...");
            } else {
                app.follow_chat_bottom();
                app.input.clear();
                app.cursor_pos = 0;
                app.sync_model_picker_preview_from_input();
            }
        }
        _ => {}
    }

    Ok(())
}
