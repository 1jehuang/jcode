use super::{
    App, DisplayMessage, ProcessingStatus, SendAction, ctrl_bracket_fallback_to_esc, input,
    parse_rate_limit_error, spawn_in_new_terminal,
};
use crate::bus::BusEvent;
use crate::message::ToolCall;
use crate::protocol::{NotificationType, ServerEvent, TranscriptMode};
use crate::tool::selfdev::ReloadContext;
use crate::tui::backend::{RemoteConnection, RemoteDisconnectReason, RemoteEventState, RemoteRead};
use crate::tui::ui::capitalize;
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
    pub initial_server_start: bool,
    pub last_disconnect_reason: Option<String>,
    pub server_reload_in_progress: bool,
    pub reload_recovery_attempted: bool,
    pub last_reload_pid: Option<u32>,
}

fn format_disconnect_reason(reason: &RemoteDisconnectReason) -> String {
    match reason {
        RemoteDisconnectReason::PeerClosed => "server closed the connection".to_string(),
        RemoteDisconnectReason::Io(err) => {
            let lowered = err.to_lowercase();
            if lowered.contains("connection reset") {
                "connection reset by server".to_string()
            } else if lowered.contains("broken pipe") {
                "broken pipe while talking to server".to_string()
            } else if lowered.contains("timed out") {
                "connection timed out".to_string()
            } else {
                err.clone()
            }
        }
        RemoteDisconnectReason::Protocol(err) => {
            format!("protocol error while reading server event: {}", err)
        }
    }
}

fn reconnect_status_message(app: &App, state: &RemoteRunState, detail: &str) -> String {
    let elapsed = state
        .disconnect_start
        .map(|start| start.elapsed())
        .unwrap_or_default();
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
        format!("\n  Resume later: jcode --resume {}", name)
    } else {
        String::new()
    };

    format!(
        "⚡ Connection lost — retrying (attempt {}, {})\n  Cause: {}{}",
        state.reconnect_attempts.max(1),
        elapsed_str,
        detail,
        resume_hint,
    )
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ReloadReconnectHints {
    pub has_reload_ctx_for_session: bool,
    pub has_client_reload_marker: bool,
}

const RELOAD_MARKER_MAX_AGE: Duration = Duration::from_secs(30);

fn persist_replay_display_message(app: &mut App, role: &str, title: Option<String>, content: &str) {
    if app.is_remote {
        // In remote mode, the server owns authoritative session history. Persisting the
        // client's stale shadow copy can roll back newer turns after reconnect/reload.
        return;
    }
    app.session
        .record_replay_display_message(role.to_string(), title, content.to_string());
    let _ = app.session.save();
}

fn persist_swarm_status_snapshot(app: &mut App) {
    if app.is_remote {
        // Avoid clobbering the server-owned session file from a remote client's shadow copy.
        return;
    }
    app.session
        .record_swarm_status_event(app.remote_swarm_members.clone());
    let _ = app.session.save();
}

fn persist_swarm_plan_snapshot(
    app: &mut App,
    swarm_id: String,
    version: u64,
    items: Vec<crate::plan::PlanItem>,
    participants: Vec<String>,
    reason: Option<String>,
) {
    if app.is_remote {
        // Avoid clobbering the server-owned session file from a remote client's shadow copy.
        return;
    }
    app.session
        .record_swarm_plan_event(swarm_id, version, items, participants, reason);
    let _ = app.session.save();
}

fn persist_remote_session_metadata<F>(app: &mut App, update: F) -> Result<()>
where
    F: FnOnce(&mut crate::session::Session),
{
    let session_id = app
        .remote_session_id
        .as_deref()
        .or(app.resume_session_id.as_deref())
        .unwrap_or(app.session.id.as_str());
    let mut session = crate::session::Session::load(session_id)?;
    update(&mut session);
    session.save()?;
    app.session = session;
    Ok(())
}

fn reload_marker_active() -> bool {
    crate::server::reload_marker_active(RELOAD_MARKER_MAX_AGE)
}

pub(super) fn reload_handoff_active(state: &RemoteRunState) -> bool {
    state.server_reload_in_progress || reload_marker_active()
}

async fn recover_reloading_server(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    state: &mut RemoteRunState,
    detail: &str,
) -> Result<bool> {
    if state.reload_recovery_attempted || crate::cli::dispatch::server_is_running().await {
        return Ok(false);
    }

    state.reload_recovery_attempted = true;
    state.last_disconnect_reason = Some(detail.to_string());

    let content = reconnect_status_message(app, state, detail);
    if let Some(idx) = state.disconnect_msg_idx {
        let _ = app.replace_display_message_content(idx, content);
    } else {
        app.push_display_message(DisplayMessage::system(content));
        state.disconnect_msg_idx = Some(app.display_messages.len() - 1);
    }
    terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;

    crate::logging::warn(&format!(
        "Reload reconnect failed definitively ({}); spawning a replacement shared server",
        detail
    ));

    match crate::cli::dispatch::spawn_server(&crate::cli::provider_init::ProviderChoice::Auto, None)
        .await
    {
        Ok(()) => {
            state.initial_server_start = true;
            state.last_disconnect_reason =
                Some("replacement server started; reconnecting".to_string());
            crate::logging::info("Replacement shared server started after stalled reload");
            Ok(true)
        }
        Err(error) => {
            state.last_disconnect_reason = Some(format!(
                "reload recovery failed while starting server: {}",
                error
            ));
            crate::logging::error(&format!(
                "Failed to start replacement server after reload failure: {}",
                error
            ));
            Ok(false)
        }
    }
}

pub(super) async fn handle_tick(app: &mut App, remote: &mut RemoteConnection) {
    if let Some(chunk) = app.stream_buffer.flush() {
        app.streaming_text.push_str(&chunk);
    }

    let _ = check_debug_command(app, remote).await;

    if let Some(reset_time) = app.rate_limit_reset {
        if Instant::now() >= reset_time {
            app.rate_limit_reset = None;
            if !app.is_processing {
                if let Some(pending) = app.rate_limit_pending_message.clone() {
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
                    return;
                }
            }
        }
    }

    if app.pending_queued_dispatch {
        return;
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
    }

    detect_and_cancel_stall(app, remote).await;
}

pub(super) async fn handle_terminal_event(
    app: &mut App,
    _terminal: &mut DefaultTerminal,
    remote: &mut RemoteConnection,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<()> {
    match event {
        Some(Ok(Event::FocusGained)) => {
            app.note_client_focus();
        }
        Some(Ok(Event::Key(key))) => {
            app.note_client_focus();
            app.update_copy_badge_key_event(key);
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                handle_remote_key(app, key.code, key.modifiers, remote).await?;
                if let Some(spec) = app.pending_model_switch.take() {
                    let _ = remote.set_model(&spec).await;
                }
                if let Some(selection) = app.pending_account_picker_selection.take() {
                    match selection {
                        crate::tui::AccountPickerSelection::Switch { provider_id, label } => {
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
                        crate::tui::AccountPickerSelection::Add { .. }
                        | crate::tui::AccountPickerSelection::Replace { .. } => {}
                    }
                }
            }
        }
        Some(Ok(Event::Paste(text))) => {
            app.note_client_focus();
            app.handle_paste(text);
        }
        Some(Ok(Event::Mouse(mouse))) => {
            app.note_client_focus();
            handle_mouse_event(app, mouse);
        }
        Some(Ok(Event::Resize(_, _))) => {}
        _ => {}
    }
    // The active remote loop redraws at the top of the next iteration, so an
    // immediate draw here would duplicate the same full-frame render for every
    // keypress.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{RemoteRunState, reload_handoff_active};

    #[test]
    fn reload_handoff_active_when_server_flag_is_set() {
        let state = RemoteRunState {
            server_reload_in_progress: true,
            ..RemoteRunState::default()
        };

        assert!(reload_handoff_active(&state));
    }

    #[test]
    fn reload_handoff_inactive_without_flag_or_marker() {
        assert!(!reload_handoff_active(&RemoteRunState::default()));
    }
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
        Ok(BusEvent::DictationCompleted { text, mode }) => {
            match remote.send_transcript(text, mode).await {
                Ok(()) => app.mark_dictation_delivered(),
                Err(error) => app.handle_dictation_failure(error.to_string()),
            }
        }
        Ok(BusEvent::DictationFailed { message }) => {
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

pub(super) async fn connect_with_retry(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event_stream: &mut EventStream,
    state: &mut RemoteRunState,
    session_to_resume: Option<&str>,
) -> Result<ConnectOutcome> {
    match RemoteConnection::connect_with_session(session_to_resume).await {
        Ok(remote) => {
            crate::logging::info(&format!(
                "[TIMING] remote bootstrap: connected after {}ms (resume={:?}, reconnect_attempts={})",
                app.app_started.elapsed().as_millis(),
                session_to_resume,
                state.reconnect_attempts
            ));
            if let Some(idx) = state.disconnect_msg_idx.take() {
                let _ = app.remove_display_message(idx);
            }
            state.disconnect_start = None;
            state.last_disconnect_reason = None;
            state.server_reload_in_progress = false;
            state.reload_recovery_attempted = false;
            state.last_reload_pid = None;
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
                state.initial_server_start = true;
                app.server_spawning = false;
            }
            state.reconnect_attempts += 1;
            state.disconnect_start.get_or_insert_with(Instant::now);

            let msg_content = if is_initial_server_start {
                "⏳ Starting server...".to_string()
            } else {
                let fallback_reason = e.root_cause().to_string();
                reconnect_status_message(
                    app,
                    state,
                    state
                        .last_disconnect_reason
                        .as_deref()
                        .unwrap_or(fallback_reason.as_str()),
                )
            };

            if let Some(idx) = state.disconnect_msg_idx {
                let _ = app.replace_display_message_content(idx, msg_content);
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

            if reload_handoff_active(state) {
                let socket_path = crate::server::socket_path();
                match crate::server::inspect_reload_wait_status(
                    &socket_path,
                    RELOAD_MARKER_MAX_AGE,
                    state.last_reload_pid,
                )
                .await
                {
                    crate::server::ReloadWaitStatus::Ready => {
                        crate::logging::info(&format!(
                            "Reconnect reload handoff: ready immediately (state={})",
                            crate::server::reload_state_summary(RELOAD_MARKER_MAX_AGE)
                        ));
                        return Ok(ConnectOutcome::Retry);
                    }
                    crate::server::ReloadWaitStatus::Failed(detail) => {
                        crate::logging::warn(&format!(
                            "Reconnect reload handoff: failed detail={:?} state={}",
                            detail,
                            crate::server::reload_state_summary(RELOAD_MARKER_MAX_AGE)
                        ));
                        let detail = detail.unwrap_or_else(|| {
                            "reload failed before the replacement server became ready; starting replacement server"
                                .to_string()
                        });
                        if recover_reloading_server(app, terminal, state, &detail).await? {
                            return Ok(ConnectOutcome::Retry);
                        }
                    }
                    crate::server::ReloadWaitStatus::Idle => {
                        crate::logging::warn(&format!(
                            "Reconnect reload handoff: idle without ready server state={}",
                            crate::server::reload_state_summary(RELOAD_MARKER_MAX_AGE)
                        ));
                        if recover_reloading_server(
                            app,
                            terminal,
                            state,
                            "reload ended without a ready replacement server; starting replacement server",
                        )
                        .await?
                        {
                            return Ok(ConnectOutcome::Retry);
                        }
                    }
                    crate::server::ReloadWaitStatus::Waiting { pid } => {
                        state.last_reload_pid = pid;
                        crate::logging::info(&format!(
                            "Reconnect wait: awaiting reload lifecycle event (pid={:?}, state={})",
                            pid,
                            crate::server::reload_state_summary(RELOAD_MARKER_MAX_AGE)
                        ));
                        let wait = crate::server::wait_for_reload_handoff_event(pid, &socket_path);
                        tokio::pin!(wait);
                        loop {
                            tokio::select! {
                                _ = &mut wait => break,
                                event = event_stream.next() => {
                                    if handle_terminal_event_while_disconnected(
                                        app,
                                        terminal,
                                        event,
                                    )? {
                                        return Ok(ConnectOutcome::Quit);
                                    }
                                }
                            }
                        }
                        return Ok(ConnectOutcome::Retry);
                    }
                }
            }

            {
                let backoff = if state.initial_server_start && state.reconnect_attempts <= 20 {
                    Duration::from_millis(100)
                } else if state.reconnect_attempts <= 2 {
                    Duration::from_millis(100)
                } else {
                    if state.initial_server_start {
                        state.initial_server_start = false;
                    }
                    Duration::from_secs((1u64 << (state.reconnect_attempts - 2).min(5)).min(30))
                };
                let sleep = tokio::time::sleep(backoff);
                tokio::pin!(sleep);
                loop {
                    tokio::select! {
                        _ = &mut sleep => break,
                        event = event_stream.next() => {
                            if handle_terminal_event_while_disconnected(
                                app,
                                terminal,
                                event,
                            )? {
                                return Ok(ConnectOutcome::Quit);
                            }
                        }
                    }
                }
            }

            Ok(ConnectOutcome::Retry)
        }
    }
}

fn handle_terminal_event_while_disconnected(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<bool> {
    let mut needs_redraw = false;

    match event {
        Some(Ok(Event::FocusGained)) => {
            app.note_client_focus();
        }
        Some(Ok(Event::Key(key))) => {
            app.note_client_focus();
            app.update_copy_badge_key_event(key);
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                handle_disconnected_key(app, key.code, key.modifiers)?;
            }
            needs_redraw = true;
        }
        Some(Ok(Event::Paste(text))) => {
            app.note_client_focus();
            app.handle_paste(text);
            needs_redraw = true;
        }
        Some(Ok(Event::Mouse(mouse))) => {
            app.note_client_focus();
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

pub(super) async fn handle_post_connect<B: ratatui::backend::Backend>(
    app: &mut App,
    terminal: &mut ratatui::Terminal<B>,
    remote: &mut RemoteConnection,
    state: &mut RemoteRunState,
    session_to_resume: Option<&str>,
) -> Result<PostConnectOutcome> {
    crate::logging::info(&format!(
        "Reload check: session_to_resume={:?}, remote_session_id={:?}, reconnect_attempts={}",
        session_to_resume, app.remote_session_id, state.reconnect_attempts
    ));
    let hints = load_reload_reconnect_hints(app, session_to_resume);
    let has_reload_ctx_for_session = hints.has_reload_ctx_for_session;
    if state.reconnect_attempts > 0 {
        if let Some(disconnect_start) = state.disconnect_start {
            crate::logging::info(&format!(
                "Reload reconnect succeeded after {}ms (attempts={})",
                disconnect_start.elapsed().as_millis(),
                state.reconnect_attempts
            ));
        }
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

    finalize_reload_reconnect(app, session_to_resume, hints, state.reconnect_attempts > 0);

    let same_session_reload_fast_path = state.reconnect_attempts > 0
        && session_to_resume
            .zip(app.remote_session_id.as_deref())
            .map(|(resume_id, remote_id)| resume_id == remote_id)
            .unwrap_or(false)
        && !app.display_messages.is_empty();

    state.reconnect_attempts = 0;
    state.initial_server_start = false;

    if same_session_reload_fast_path {
        crate::logging::info(
            "Same-session reload fast path: skipping blocking History wait and reusing local display state",
        );
        remote.mark_history_loaded();
    } else if !remote.has_loaded_history() {
        app.set_status_notice("Loading session...");
    }

    if remote.has_loaded_history() && !app.is_processing && !app.queued_messages.is_empty() {
        crate::logging::info(
            "Post-connect history restored with queued continuation; dispatching immediately",
        );
        process_remote_followups(app, remote).await;
    }

    Ok(PostConnectOutcome::Ready)
}

pub(super) fn load_reload_reconnect_hints(
    app: &mut App,
    session_to_resume: Option<&str>,
) -> ReloadReconnectHints {
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

    ReloadReconnectHints {
        has_reload_ctx_for_session,
        has_client_reload_marker,
    }
}

pub(super) fn finalize_reload_reconnect(
    app: &mut App,
    session_to_resume: Option<&str>,
    hints: ReloadReconnectHints,
    reconnected_after_disconnect: bool,
) {
    let should_queue_reload_continuation = hints.has_reload_ctx_for_session;
    crate::logging::info(&format!(
        "Reload continuation check: should_queue={}, reload_info_empty={}, has_ctx={}, has_marker={}",
        should_queue_reload_continuation,
        app.reload_info.is_empty(),
        hints.has_reload_ctx_for_session,
        hints.has_client_reload_marker
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
            let background_task_note = session_to_resume
                .map(super::reload_persisted_background_tasks_note)
                .unwrap_or_default();

            let continuation_msg = format!(
                "Reload succeeded ({} → {}).{}{} Continue immediately from where you left off. Do not ask the user what to do next. Do not summarize the reload.",
                ctx.version_before, ctx.version_after, task_info, background_task_note
            );

            crate::logging::info(&format!(
                "Queuing reload continuation message ({} chars)",
                continuation_msg.len()
            ));
            app.push_display_message(DisplayMessage::system("Reload complete — continuing."));
            app.hidden_queued_system_messages.push(continuation_msg);
        } else {
            crate::logging::warn(
                "Reload context missing for initiating session after reconnect; skipping selfdev continuation",
            );
        }
        app.reload_info.clear();
    } else if hints.has_client_reload_marker {
        // A client re-exec marker only tells us to surface reload status to the UI.
        // It does not imply that this session initiated a selfdev reload or that a
        // persisted ReloadContext exists. Non-initiating clients rely on the History
        // payload's `was_interrupted` flag for generic continuation handling.
        if !reconnected_after_disconnect && !app.reload_info.is_empty() {
            app.push_display_message(DisplayMessage::system(app.reload_info.join("\n")));
        }
        app.reload_info.clear();
    }
}

pub(super) async fn handle_remote_event(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    remote: &mut RemoteConnection,
    state: &mut RemoteRunState,
    event: RemoteRead,
) -> Result<RemoteEventOutcome> {
    match event {
        RemoteRead::Disconnected(reason) => {
            handle_disconnect(app, state, Some(reason));
            terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
            Ok(RemoteEventOutcome::Reconnect)
        }
        RemoteRead::Event(ServerEvent::Reloading { new_socket }) => {
            let _ = new_socket;
            state.server_reload_in_progress = true;
            state.reload_recovery_attempted = false;
            state.last_disconnect_reason = Some("server reload in progress".to_string());
            let _ = handle_server_event(app, ServerEvent::Reloading { new_socket: None }, remote);
            process_remote_followups(app, remote).await;
            Ok(RemoteEventOutcome::Continue)
        }
        RemoteRead::Event(ServerEvent::ClientDebugRequest { id, command }) => {
            let output = handle_debug_command(app, &command, remote).await;
            let _ = remote.send_client_debug_response(id, output).await;
            process_remote_followups(app, remote).await;
            Ok(RemoteEventOutcome::Continue)
        }
        RemoteRead::Event(server_event) => {
            let _ = handle_server_event(app, server_event, remote);
            process_remote_followups(app, remote).await;
            Ok(RemoteEventOutcome::Continue)
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
    state.last_disconnect_reason = Some(detail.clone());

    let scheduled_retry =
        app.schedule_pending_remote_retry(&format!("⚡ Connection lost ({detail})."));
    if !scheduled_retry {
        app.clear_pending_remote_retry();
    }
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
    app.batch_progress = None;
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.streaming_tps_start = None;
    app.streaming_tps_elapsed = Duration::ZERO;
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    state.disconnect_start = Some(Instant::now());
    state.reconnect_attempts = state.reconnect_attempts.max(1);
    state.reload_recovery_attempted = false;
    app.push_display_message(DisplayMessage {
        role: "system".to_string(),
        content: reconnect_status_message(app, state, &detail),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: None,
    });
    state.disconnect_msg_idx = Some(app.display_messages.len() - 1);
    state.reconnect_attempts = 1;
}

pub(super) async fn process_remote_followups(app: &mut App, remote: &mut RemoteConnection) {
    if app.pending_queued_dispatch {
        return;
    }

    if !remote.has_loaded_history() {
        return;
    }

    if app.pending_server_reload && !app.is_processing {
        app.pending_server_reload = false;
        app.push_display_message(DisplayMessage::system(
            "ℹ Newer server binary detected. Automatic reload is disabled to avoid interrupting other attached clients. Use `/reload` manually when you're ready.".to_string(),
            ));
        app.set_status_notice("Server update available — manual /reload recommended");
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
        let auto_retry = reminder.is_some() && messages.is_empty();
        for msg in display_system_messages {
            app.push_display_message(DisplayMessage::system(msg));
        }
        for msg in &messages {
            app.push_display_message(DisplayMessage::user(msg.clone()));
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

pub(super) async fn begin_remote_send(
    app: &mut App,
    remote: &mut RemoteConnection,
    content: String,
    images: Vec<(String, String)>,
    is_system: bool,
    system_reminder: Option<String>,
    auto_retry: bool,
    retry_attempts: u8,
) -> Result<u64> {
    let msg_id = remote
        .send_message_with_images_and_reminder(
            content.clone(),
            images.clone(),
            system_reminder.clone(),
        )
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
        system_reminder,
        auto_retry,
        retry_attempts,
        retry_at: None,
    });
    remote.reset_call_output_tokens_seen();
    Ok(msg_id)
}

fn set_transcript_input(app: &mut App, text: String) {
    app.input = text;
    app.cursor_pos = app.input.len();
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
}

fn queue_transcript_input(app: &mut App) {
    input::queue_message(app);
    let count = app.queued_messages.len();
    app.set_status_notice(format!(
        "Transcript queued ({} message{})",
        count,
        if count == 1 { "" } else { "s" }
    ));
}

fn submit_transcript_input(app: &mut App) {
    match app.send_action(false) {
        SendAction::Submit => app.submit_input(),
        SendAction::Queue => queue_transcript_input(app),
        SendAction::Interleave => {
            let prepared = input::take_prepared_input(app);
            input::stage_local_interleave(app, prepared.expanded);
        }
    }
}

async fn submit_remote_input_shell(
    app: &mut App,
    remote: &mut RemoteConnection,
    raw_input: String,
    command: String,
) -> Result<()> {
    app.push_display_message(DisplayMessage::user(raw_input));

    if command.trim().is_empty() {
        app.push_display_message(DisplayMessage::system(
            "Shell command cannot be empty after `!`.",
        ));
        app.set_status_notice("Shell command is empty");
        return Ok(());
    }

    let request_id = remote.send_input_shell(command.clone()).await?;
    app.current_message_id = Some(request_id);
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
    app.rate_limit_pending_message = None;
    remote.reset_call_output_tokens_seen();
    app.set_status_notice(format!(
        "Running remote shell: {}",
        crate::util::truncate_str(&command, 48)
    ));
    Ok(())
}

pub(super) fn apply_transcript_event(app: &mut App, text: String, mode: TranscriptMode) {
    if text.trim().is_empty() {
        app.set_status_notice("Transcript was empty");
        return;
    }

    match mode {
        TranscriptMode::Insert => {
            input::insert_input_text(app, &text);
            app.set_status_notice("Transcript inserted");
        }
        TranscriptMode::Append => {
            let mut combined = app.input.clone();
            combined.push_str(&text);
            set_transcript_input(app, combined);
            app.set_status_notice("Transcript appended");
        }
        TranscriptMode::Replace => {
            set_transcript_input(app, text);
            app.set_status_notice("Transcript replaced input");
        }
        TranscriptMode::Send => {
            input::insert_input_text(app, &text);
            submit_transcript_input(app);
        }
    }

    app.follow_chat_bottom_for_typing();
}

pub(super) fn handle_server_event(
    app: &mut App,
    event: ServerEvent,
    remote: &mut impl RemoteEventState,
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
            app.commit_pending_streaming_assistant_message();
            if matches!(name.as_str(), "memory") {
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
            app.commit_pending_streaming_assistant_message();
            crate::tui::mermaid::clear_streaming_preview_diagram();
            let is_batch = name == "batch";
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
            if is_batch {
                app.batch_progress = None;
            }
            app.streaming_tool_calls.clear();
            app.status = ProcessingStatus::Streaming;
            true
        }
        ServerEvent::BatchProgress { progress } => {
            app.batch_progress = Some(progress);
            false
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
            app.update_terminal_title();
            false
        }
        ServerEvent::Pong { .. } => false,
        ServerEvent::ConnectionPhase { phase } => {
            let cp = match phase.as_str() {
                "authenticating" => crate::message::ConnectionPhase::Authenticating,
                "connecting" => crate::message::ConnectionPhase::Connecting,
                "waiting for response" => crate::message::ConnectionPhase::WaitingForResponse,
                "streaming" => crate::message::ConnectionPhase::Streaming,
                _ if phase.starts_with("retrying (") && phase.ends_with(')') => {
                    let inner = &phase[10..phase.len() - 1];
                    let (attempt, max) = inner
                        .split_once('/')
                        .and_then(|(a, m)| Some((a.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
                        .unwrap_or((1, 1));
                    crate::message::ConnectionPhase::Retrying { attempt, max }
                }
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
            let keep_pending_retry = app
                .rate_limit_pending_message
                .as_ref()
                .is_some_and(|pending| pending.auto_retry && app.rate_limit_reset.is_some());
            if !keep_pending_retry {
                app.clear_pending_remote_retry();
            }
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
            app.batch_progress = None;
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            app.schedule_queued_dispatch_after_interrupt();
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
                app.clear_pending_remote_retry();
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
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.processing_started = None;
                app.replay_processing_started_ms = None;
                app.replay_elapsed_override = None;
                app.batch_progress = None;
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
            if !app.schedule_pending_remote_retry("⚠ Remote request failed.") {
                app.clear_pending_remote_retry();
            }
            false
        }
        ServerEvent::SessionId { session_id } => {
            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            app.note_client_focus();
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
            images,
            session_id,
            provider_name,
            provider_model,
            subagent_model,
            available_models,
            available_model_routes,
            mcp_servers,
            skills,
            all_sessions,
            client_count,
            is_canary,
            server_version,
            server_name,
            server_icon,
            server_has_update,
            was_interrupted,
            connection_type,
            upstream_provider,
            reasoning_effort,
            service_tier,
            compaction_mode,
            side_panel,
            ..
        } => {
            let prev_session_id = app.remote_session_id.clone();
            let history_message_count = messages.len();
            let history_mcp_count = mcp_servers.len();
            let history_model = provider_model.clone();
            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            app.note_client_focus();
            let session_changed = prev_session_id.as_deref() != Some(session_id.as_str());

            if session_changed {
                app.rate_limit_pending_message = None;
                app.rate_limit_reset = None;
                app.connection_type = None;
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
                app.remote_side_pane_images.clear();
                app.remote_swarm_members.clear();
                app.swarm_plan_items.clear();
                app.swarm_plan_version = None;
                app.swarm_plan_swarm_id = None;
                remote.reset_call_output_tokens_seen();
            }
            if let Some(name) = provider_name {
                app.remote_provider_name = Some(name);
            }
            if let Some(model) = provider_model {
                app.update_context_limit_for_model(&model);
                app.remote_provider_model = Some(model);
            }
            app.session.subagent_model = subagent_model;
            if upstream_provider.is_some() {
                app.upstream_provider = upstream_provider;
            }
            if session_changed || connection_type.is_some() {
                app.connection_type = connection_type;
            }
            app.remote_reasoning_effort = reasoning_effort;
            app.remote_service_tier = service_tier;
            app.remote_compaction_mode = Some(compaction_mode);
            app.set_side_panel_snapshot(side_panel);
            app.remote_side_pane_images = images;
            app.remote_available_models = available_models;
            app.remote_model_routes = available_model_routes;
            app.remote_skills = skills;
            app.remote_sessions = all_sessions;
            app.remote_client_count = client_count;
            app.remote_is_canary = is_canary;
            app.remote_server_version = server_version;
            app.remote_server_has_update = server_has_update;

            if server_has_update == Some(true) && !app.pending_server_reload {
                app.pending_server_reload = true;
                app.set_status_notice("Server update available");
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

            let should_apply_history_payload = session_changed || !remote.has_loaded_history();
            if should_apply_history_payload {
                crate::logging::info(&format!(
                    "[TIMING] remote bootstrap: history after {}ms (session={}, resumed={}, messages={}, mcp_servers={}, model={})",
                    app.app_started.elapsed().as_millis(),
                    session_id,
                    app.resume_session_id.is_some(),
                    history_message_count,
                    history_mcp_count,
                    history_model.as_deref().unwrap_or("<none>")
                ));
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
            } else {
                crate::logging::info(
                    "Ignoring duplicate History event for active session after local state was restored",
                );
            }

            if was_interrupted == Some(true) && !app.display_messages.is_empty() {
                crate::logging::info(
                    "Session was interrupted mid-generation, queuing continuation",
                );
                app.push_display_message(DisplayMessage::system(
                    "Reload complete — continuing.".to_string(),
                ));
                app.hidden_queued_system_messages.push(
                    "Your session was interrupted by a server reload while you were working. The session has been restored. Any tool that was running was aborted and its results may be incomplete. Continue exactly where you left off and do not ask the user what to do next."
                        .to_string(),
                );
            }

            false
        }
        ServerEvent::SidePanelState { snapshot } => {
            app.set_side_panel_snapshot(snapshot);
            false
        }
        ServerEvent::SwarmStatus { members } => {
            if app.swarm_enabled {
                app.remote_swarm_members = members;
                persist_swarm_status_snapshot(app);
            } else {
                app.remote_swarm_members.clear();
            }
            false
        }
        ServerEvent::SwarmPlan {
            swarm_id,
            version,
            items,
            participants,
            reason,
        } => {
            app.swarm_plan_swarm_id = Some(swarm_id.clone());
            app.swarm_plan_version = Some(version);
            app.swarm_plan_items = items.clone();
            persist_swarm_plan_snapshot(app, swarm_id, version, items, participants, reason);
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
            let message = format!(
                "Plan proposal received in swarm {}\nFrom: {}\nSummary: {}",
                swarm_id, proposer, summary
            );
            app.push_display_message(DisplayMessage::system(message.clone()));
            persist_replay_display_message(app, "system", None, &message);
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
        ServerEvent::ReasoningEffortChanged { effort, error, .. } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set effort: {}",
                    err
                )));
            } else {
                app.remote_reasoning_effort = effort.clone();
                let label = effort
                    .as_deref()
                    .map(super::effort_display_label)
                    .unwrap_or("default");
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Reasoning effort → {}",
                    label
                )));
                app.set_status_notice(format!("Effort: {}", label));
            }
            false
        }
        ServerEvent::ServiceTierChanged {
            service_tier,
            error,
            ..
        } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set fast mode: {}",
                    err
                )));
            } else {
                app.remote_service_tier = service_tier.clone();
                let enabled = service_tier.as_deref() == Some("priority");
                let label = service_tier
                    .as_deref()
                    .map(super::service_tier_display_label)
                    .unwrap_or("Standard");
                let applies_next_request = app.is_processing;
                app.push_display_message(DisplayMessage::system(super::fast_mode_success_message(
                    enabled,
                    label,
                    applies_next_request,
                )));
                app.set_status_notice(super::fast_mode_status_notice(
                    enabled,
                    applies_next_request,
                ));
            }
            false
        }
        ServerEvent::TransportChanged {
            transport, error, ..
        } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set transport: {}",
                    err
                )));
            } else {
                app.remote_transport = transport.clone();
                let label = transport.as_deref().unwrap_or("unknown");
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Transport → {}",
                    label
                )));
                app.set_status_notice(format!("Transport: {}", label));
            }
            false
        }
        ServerEvent::CompactionModeChanged { mode, error, .. } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set compaction mode: {}",
                    err
                )));
            } else {
                let label = mode.as_str();
                app.remote_compaction_mode = Some(mode);
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Compaction mode → {}",
                    label
                )));
                app.set_status_notice(format!("Compaction: {}", label));
            }
            false
        }
        ServerEvent::SoftInterruptInjected {
            content,
            display_role,
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
            let role = display_role.unwrap_or_else(|| "user".to_string());
            app.push_display_message(DisplayMessage {
                role,
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
            display_prompt,
            prompt_chars: _,
            computed_age_ms,
        } => {
            if app.memory_enabled {
                let plural = if count == 1 { "memory" } else { "memories" };
                let display_prompt = if let Some(display_prompt) = display_prompt {
                    display_prompt.clone()
                } else if prompt.trim().is_empty() {
                    "# Memory\n\n## Notes\n1. (content unavailable from server event)".to_string()
                } else {
                    prompt.clone()
                };
                crate::memory::record_injected_prompt(&prompt, count, computed_age_ms);
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
        ServerEvent::Notification {
            from_session,
            from_name,
            notification_type,
            message,
        } => {
            let sender = from_name
                .clone()
                .or_else(|| crate::id::extract_session_name(&from_session).map(str::to_string))
                .unwrap_or_else(|| from_session[..8.min(from_session.len())].to_string());

            let (title, message, status_notice) = match notification_type {
                NotificationType::Message { scope, channel } => {
                    let title = match scope.as_deref() {
                        Some("dm") => format!("DM from {}", sender),
                        Some("channel") => format!(
                            "#{} · {}",
                            channel.unwrap_or_else(|| "channel".to_string()),
                            sender
                        ),
                        Some("broadcast") => format!("Broadcast · {}", sender),
                        Some("plan") => format!("Plan update · {}", sender),
                        Some("swarm") => format!("Swarm · {}", sender),
                        Some(other) => format!("{} · {}", capitalize(other), sender),
                        None => format!("Swarm · {}", sender),
                    };
                    (
                        title,
                        message.trim().to_string(),
                        "New swarm message".to_string(),
                    )
                }
                NotificationType::SharedContext { key, value } => (
                    format!("Shared context · {}", sender),
                    format!("{} = {}", key, value).trim().to_string(),
                    format!("Shared context: {}", key),
                ),
                NotificationType::FileConflict { path, operation } => (
                    format!("File activity · {}", sender),
                    format!("{} {}", operation, path).trim().to_string(),
                    format!("{} {}", operation, path),
                ),
            };

            app.push_display_message(DisplayMessage::swarm(title.clone(), message.clone()));
            persist_replay_display_message(app, "swarm", Some(title), &message);
            app.set_status_notice(status_notice);
            false
        }
        ServerEvent::Transcript { text, mode } => {
            apply_transcript_event(app, text, mode);
            false
        }
        ServerEvent::InputShellResult { result } => {
            app.push_display_message(DisplayMessage::system(
                crate::message::format_input_shell_result_markdown(&result),
            ));
            app.set_status_notice(crate::message::input_shell_status_notice(&result));
            false
        }
        ServerEvent::Compaction {
            trigger,
            pre_tokens,
            post_tokens,
            tokens_saved,
            duration_ms,
            messages_dropped,
            messages_compacted,
            summary_chars,
            active_messages,
        } => {
            app.handle_compaction_event(crate::compaction::CompactionEvent {
                trigger,
                pre_tokens,
                post_tokens,
                tokens_saved,
                duration_ms,
                messages_dropped,
                messages_compacted,
                summary_chars,
                active_messages,
            });
            false
        }
        ServerEvent::SplitResponse {
            new_session_id,
            new_session_name,
            ..
        } => {
            let exe = std::env::current_exe().unwrap_or_default();
            let cwd = crate::session::Session::load(&new_session_id)
                .ok()
                .and_then(|session| session.working_dir)
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_dir())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
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
                app.set_status_notice("Compacting context");
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
                    app.remember_input_undo_state();
                    app.input = prompt.clone();
                    app.cursor_pos = app.input.len();
                    app.follow_chat_bottom_for_typing();
                    return;
                }
            }
        }
    }
    input::insert_input_text(app, &c.to_string());
    app.follow_chat_bottom_for_typing();
}

fn handle_disconnected_local_command(app: &mut App, trimmed: &str) -> bool {
    let handled = super::commands::handle_help_command(app, trimmed)
        || super::commands::handle_session_command(app, trimmed)
        || super::commands::handle_goals_command(app, trimmed)
        || super::commands::handle_config_command(app, trimmed)
        || super::state_ui::handle_info_command(app, trimmed)
        || super::auth::handle_auth_command(app, trimmed)
        || (trimmed == "/restart" && super::tui_lifecycle::handle_dev_command(app, trimmed));

    if handled {
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

pub(super) fn handle_disconnected_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
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

impl App {
    pub(super) async fn handle_account_picker_command_remote(
        &mut self,
        remote: &mut RemoteConnection,
        command: crate::tui::account_picker::AccountPickerCommand,
    ) -> Result<()> {
        match command {
            crate::tui::account_picker::AccountPickerCommand::OpenAccountCenter {
                provider_filter,
            } => self.open_account_center(provider_filter.as_deref()),
            crate::tui::account_picker::AccountPickerCommand::OpenAddReplaceFlow {
                provider_filter,
            } => self.open_account_add_replace_flow(provider_filter.as_deref()),
            crate::tui::account_picker::AccountPickerCommand::SubmitInput(input) => {
                crate::tui::app::auth::handle_account_command_remote(self, &input, remote).await?;
            }
            crate::tui::account_picker::AccountPickerCommand::PromptValue {
                prompt,
                command_prefix,
                empty_value,
                status_notice,
            } => self.prompt_account_value(prompt, command_prefix, empty_value, status_notice),
            crate::tui::account_picker::AccountPickerCommand::PromptNew { provider } => {
                self.prompt_new_account_label(provider)
            }
            other => {
                if let Some(input) = Self::account_command_for_picker(&other) {
                    crate::tui::app::auth::handle_account_command_remote(self, &input, remote)
                        .await?;
                }
            }
        }
        Ok(())
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

    if app.login_picker_overlay.is_some() {
        return app.handle_login_picker_key(code, modifiers);
    }

    if app.account_picker_overlay.is_some() {
        if let Some(command) = app.next_account_picker_action(code, modifiers)? {
            app.handle_account_picker_command_remote(remote, command)
                .await?;
        }
        return Ok(());
    }

    if let Some(ref picker) = app.picker_state {
        if !picker.preview {
            return app.handle_picker_key(code, modifiers);
        }
    }

    if app.handle_picker_preview_key(&code, modifiers)? {
        return Ok(());
    }

    if input::handle_visible_copy_shortcut(app, code, modifiers) {
        return Ok(());
    }

    if app.dictation_key_matches(code, modifiers) {
        app.handle_dictation_trigger();
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
    if let Some(direction) = app.model_switch_keys.direction_for(code, modifiers) {
        remote.cycle_model(direction).await?;
        return Ok(());
    }
    if let Some(direction) = app.effort_switch_keys.direction_for(code, modifiers) {
        let efforts = ["none", "low", "medium", "high", "xhigh"];
        let current = app.remote_reasoning_effort.as_deref();
        let current_index = current
            .and_then(|c| efforts.iter().position(|e| *e == c))
            .unwrap_or(efforts.len() - 1);
        let len = efforts.len();
        let next_index = if direction > 0 {
            if current_index + 1 >= len {
                current_index
            } else {
                current_index + 1
            }
        } else if current_index == 0 {
            0
        } else {
            current_index - 1
        };
        let next_effort = efforts[next_index];
        if Some(next_effort) == current {
            let label = super::effort_display_label(next_effort);
            app.set_status_notice(format!(
                "Effort: {} (already at {})",
                label,
                if direction > 0 { "max" } else { "min" }
            ));
        } else {
            remote.set_reasoning_effort(next_effort).await?;
        }
        return Ok(());
    }
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('s')) {
        app.toggle_typing_scroll_lock();
        return Ok(());
    }
    if app.centered_toggle_keys.toggle.matches(code, modifiers) {
        app.toggle_centered_mode();
        return Ok(());
    }
    app.normalize_diagram_state();
    let diagram_available = app.diagram_available();
    if app.handle_diagram_focus_key(code, modifiers, diagram_available) {
        return Ok(());
    }
    if app.handle_diff_pane_focus_key(code, modifiers) {
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
                if app.cursor_pos < end {
                    app.remember_input_undo_state();
                }
                app.input.drain(app.cursor_pos..end);
                return Ok(());
            }
            KeyCode::Backspace => {
                let start = app.find_word_boundary_back();
                if start < app.cursor_pos {
                    app.remember_input_undo_state();
                }
                app.input.drain(start..app.cursor_pos);
                app.cursor_pos = start;
                return Ok(());
            }
            KeyCode::Char('v') => {
                app.paste_from_clipboard();
                return Ok(());
            }
            _ => {}
        }
    }

    if let Some(amount) = app.scroll_keys.scroll_amount(code, modifiers) {
        if amount < 0 {
            app.scroll_up((-amount) as usize);
        } else {
            app.scroll_down(amount as usize);
        }
        return Ok(());
    }

    if let Some(dir) = app.scroll_keys.prompt_jump(code, modifiers) {
        if dir < 0 {
            app.scroll_to_prev_prompt();
        } else {
            app.scroll_to_next_prompt();
        }
        return Ok(());
    }

    if let Some(ratio) = App::ctrl_side_panel_ratio_preset(&code, modifiers) {
        app.set_side_panel_ratio_preset(ratio);
        return Ok(());
    }

    if let Some(rank) = App::ctrl_prompt_rank(&code, modifiers) {
        app.scroll_to_recent_prompt_rank(rank);
        return Ok(());
    }

    if app.centered_toggle_keys.toggle.matches(code, modifiers) {
        app.toggle_centered_mode();
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('s')) {
        app.toggle_typing_scroll_lock();
        return Ok(());
    }

    if app.scroll_keys.is_bookmark(code, modifiers) {
        app.toggle_scroll_bookmark();
        return Ok(());
    }

    if code == KeyCode::BackTab {
        app.diff_mode = app.diff_mode.cycle();
        if !app.diff_pane_visible() {
            app.diff_pane_focus = false;
        }
        let status = format!("Diffs: {}", app.diff_mode.label());
        app.set_status_notice(&status);
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        if app.handle_diagram_ctrl_key(code, diagram_available) {
            return Ok(());
        }
        match code {
            KeyCode::Char('b') => {
                if matches!(app.status, ProcessingStatus::RunningTool(_)) {
                    remote.background_tool().await?;
                    app.set_status_notice("Moving tool to background...");
                    return Ok(());
                }
                if app.cursor_pos > 0 {
                    app.cursor_pos = app.find_word_boundary_back();
                }
                return Ok(());
            }
            KeyCode::Char('c') | KeyCode::Char('d') => {
                if app.is_processing {
                    remote.cancel().await?;
                    app.set_status_notice("Interrupting...");
                } else {
                    app.handle_quit_request();
                }
                return Ok(());
            }
            KeyCode::Char('r') => {
                app.recover_session_without_tools();
                return Ok(());
            }
            KeyCode::Char('l') => {
                return Ok(());
            }
            KeyCode::Char('u') => {
                if app.cursor_pos > 0 {
                    app.remember_input_undo_state();
                }
                app.input.drain(..app.cursor_pos);
                app.cursor_pos = 0;
                return Ok(());
            }
            KeyCode::Char('k') => {
                if app.cursor_pos < app.input.len() {
                    app.remember_input_undo_state();
                }
                app.input.truncate(app.cursor_pos);
                return Ok(());
            }
            KeyCode::Char('z') => {
                app.undo_input_change();
                return Ok(());
            }
            KeyCode::Char('x') => {
                input::cut_input_line_to_clipboard(app);
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
            KeyCode::Char('f') => {
                if app.cursor_pos < app.input.len() {
                    app.cursor_pos = app.find_word_boundary_forward();
                }
                return Ok(());
            }
            KeyCode::Left => {
                if app.cursor_pos > 0 {
                    app.cursor_pos = app.find_word_boundary_back();
                }
                return Ok(());
            }
            KeyCode::Right => {
                if app.cursor_pos < app.input.len() {
                    app.cursor_pos = app.find_word_boundary_forward();
                }
                return Ok(());
            }
            KeyCode::Char('w') | KeyCode::Backspace => {
                let start = app.find_word_boundary_back();
                if start < app.cursor_pos {
                    app.remember_input_undo_state();
                }
                app.input.drain(start..app.cursor_pos);
                app.cursor_pos = start;
                app.sync_model_picker_preview_from_input();
                return Ok(());
            }
            KeyCode::Char('s') => {
                app.toggle_input_stash();
                return Ok(());
            }
            KeyCode::Char('v') => {
                app.paste_from_clipboard();
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

    if code == KeyCode::Enter
        && modifiers.contains(KeyModifiers::CONTROL)
        && !app.input.trim().starts_with('/')
    {
        if app.activate_model_picker_from_preview() {
            return Ok(());
        }

        if !app.input.is_empty() {
            let prepared = input::take_prepared_input(app);

            match app.send_action(true) {
                SendAction::Submit => {
                    app.push_display_message(DisplayMessage {
                        role: "user".to_string(),
                        content: prepared.raw_input,
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    let _ = app
                        .begin_remote_send(remote, prepared.expanded, prepared.images, false)
                        .await;
                }
                SendAction::Queue => {
                    app.queued_messages.push(prepared.expanded);
                }
                SendAction::Interleave => {
                    app.send_interleave_now(prepared.expanded, remote).await;
                }
            }
        }
        return Ok(());
    }

    if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
        input::insert_input_text(app, "\n");
        app.follow_chat_bottom_for_typing();
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
                let prepared = input::take_prepared_input(app);
                let trimmed = prepared.expanded.trim();

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

                if super::commands::handle_dictation_command(app, trimmed) {
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
                    crate::telemetry::end_session_with_reason(
                        app.provider.name(),
                        &app.provider.model(),
                        crate::telemetry::SessionEndReason::NormalExit,
                    );
                    // In remote mode the shared server owns session lifecycle persistence.
                    // Exiting this client should not overwrite the server's session file.
                    app.should_quit = true;
                    return Ok(());
                }

                if trimmed == "/model" || trimmed == "/models" {
                    app.open_model_picker();
                    return Ok(());
                }

                if trimmed.starts_with("/subagent-model") {
                    let rest = trimmed
                        .strip_prefix("/subagent-model")
                        .unwrap_or_default()
                        .trim();
                    if rest.is_empty() || matches!(rest, "show" | "status") {
                        let current_model = app
                            .remote_provider_model
                            .clone()
                            .unwrap_or_else(|| app.provider.model());
                        let summary = match app.session.subagent_model.as_deref() {
                            Some(model) => format!("fixed `{}`", model),
                            None => format!("inherit current (`{}`)", current_model),
                        };
                        app.push_display_message(DisplayMessage::system(format!(
                            "Subagent model for this session: {}\n\nUse `/subagent-model <name>` to pin a model, or `/subagent-model inherit` to use the current model.",
                            summary
                        )));
                        return Ok(());
                    }
                    if matches!(rest, "inherit" | "reset" | "clear") {
                        let current_model = app
                            .remote_provider_model
                            .clone()
                            .unwrap_or_else(|| app.provider.model());
                        remote.set_subagent_model(None).await?;
                        app.session.subagent_model = None;
                        app.push_display_message(DisplayMessage::system(format!(
                            "Subagent model reset to inherit the current model (`{}`).",
                            current_model
                        )));
                        app.set_status_notice("Subagent model: inherit");
                        return Ok(());
                    }
                    remote.set_subagent_model(Some(rest.to_string())).await?;
                    app.session.subagent_model = Some(rest.to_string());
                    app.push_display_message(DisplayMessage::system(format!(
                        "Subagent model pinned to `{}` for this session.",
                        rest
                    )));
                    app.set_status_notice(format!("Subagent model → {}", rest));
                    return Ok(());
                }

                if trimmed.starts_with("/subagent") {
                    let rest = trimmed.strip_prefix("/subagent").unwrap_or_default().trim();
                    if rest.is_empty() {
                        app.push_display_message(DisplayMessage::error(
                            "Usage: `/subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>`",
                        ));
                        return Ok(());
                    }
                    match super::commands::parse_manual_subagent_spec(rest) {
                        Ok(spec) => {
                            remote
                                .run_subagent(
                                    spec.prompt,
                                    spec.subagent_type,
                                    spec.model,
                                    spec.session_id,
                                )
                                .await?;
                            app.subagent_status = Some("starting subagent".to_string());
                            app.set_status_notice("Running subagent");
                        }
                        Err(error) => {
                            app.push_display_message(DisplayMessage::error(format!(
                                "{}\nUsage: `/subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>`",
                                error
                            )));
                        }
                    }
                    return Ok(());
                }

                if let Some(model_name) = trimmed.strip_prefix("/model ") {
                    let model_name = model_name.trim();
                    if model_name.is_empty() {
                        app.push_display_message(DisplayMessage::error("Usage: /model <name>"));
                        return Ok(());
                    }
                    app.upstream_provider = None;
                    remote.set_model(model_name).await?;
                    return Ok(());
                }

                if trimmed == "/effort" {
                    let current = app.remote_reasoning_effort.as_deref();
                    let label = current
                        .map(super::effort_display_label)
                        .unwrap_or("default");
                    let efforts = ["none", "low", "medium", "high", "xhigh"];
                    let list: Vec<String> = efforts
                        .iter()
                        .map(|e| {
                            if Some(*e) == current {
                                format!("**{}** ← current", super::effort_display_label(e))
                            } else {
                                super::effort_display_label(e).to_string()
                            }
                        })
                        .collect();
                    app.push_display_message(DisplayMessage::system(format!(
                        "Reasoning effort: {}\nAvailable: {}\nUse `/effort <level>` or Alt+←/→ to change.",
                        label,
                        list.join(" · ")
                    )));
                    return Ok(());
                }

                if let Some(level) = trimmed.strip_prefix("/effort ") {
                    let level = level.trim();
                    if level.is_empty() {
                        app.push_display_message(DisplayMessage::error("Usage: /effort <level>"));
                        return Ok(());
                    }
                    remote.set_reasoning_effort(level).await?;
                    return Ok(());
                }

                if matches!(trimmed, "/fast default" | "/fast default status") {
                    let default_tier = crate::config::Config::load().provider.openai_service_tier;
                    let default_enabled = default_tier.as_deref() == Some("priority");
                    let default_label = default_tier
                        .as_deref()
                        .map(super::service_tier_display_label)
                        .unwrap_or("Standard");
                    app.push_display_message(DisplayMessage::system(
                        super::fast_mode_default_message(default_enabled, default_label),
                    ));
                    return Ok(());
                }

                if let Some(mode) = trimmed.strip_prefix("/fast default ") {
                    let mode = mode.trim().to_ascii_lowercase();
                    match mode.as_str() {
                        "on" => {
                            super::auth::save_openai_fast_setting_local(app, true);
                            remote.set_service_tier("priority").await?;
                        }
                        "off" => {
                            super::auth::save_openai_fast_setting_local(app, false);
                            remote.set_service_tier("off").await?;
                        }
                        "status" => {
                            let default_tier =
                                crate::config::Config::load().provider.openai_service_tier;
                            let default_enabled = default_tier.as_deref() == Some("priority");
                            let default_label = default_tier
                                .as_deref()
                                .map(super::service_tier_display_label)
                                .unwrap_or("Standard");
                            app.push_display_message(DisplayMessage::system(
                                super::fast_mode_default_message(default_enabled, default_label),
                            ));
                        }
                        _ => {
                            app.push_display_message(DisplayMessage::error(
                                "Usage: /fast default [on|off|status]",
                            ));
                        }
                    }
                    return Ok(());
                }

                if matches!(trimmed, "/fast" | "/fast status") {
                    let current = app.remote_service_tier.as_deref();
                    let enabled = current == Some("priority");
                    let current_label = current
                        .map(super::service_tier_display_label)
                        .unwrap_or("Standard");
                    let default_tier = crate::config::Config::load().provider.openai_service_tier;
                    let default_enabled = default_tier.as_deref() == Some("priority");
                    let default_label = default_tier
                        .as_deref()
                        .map(super::service_tier_display_label)
                        .unwrap_or("Standard");
                    app.push_display_message(DisplayMessage::system(
                        super::fast_mode_overview_message(
                            enabled,
                            current_label,
                            default_enabled,
                            default_label,
                        ),
                    ));
                    return Ok(());
                }

                if let Some(mode) = trimmed.strip_prefix("/fast ") {
                    let mode = mode.trim().to_ascii_lowercase();
                    let service_tier = match mode.as_str() {
                        "on" => "priority",
                        "off" => "off",
                        "status" => {
                            let current = app.remote_service_tier.as_deref();
                            let enabled = current == Some("priority");
                            let current_label = current
                                .map(super::service_tier_display_label)
                                .unwrap_or("Standard");
                            let default_tier =
                                crate::config::Config::load().provider.openai_service_tier;
                            let default_enabled = default_tier.as_deref() == Some("priority");
                            let default_label = default_tier
                                .as_deref()
                                .map(super::service_tier_display_label)
                                .unwrap_or("Standard");
                            app.push_display_message(DisplayMessage::system(
                                super::fast_mode_overview_message(
                                    enabled,
                                    current_label,
                                    default_enabled,
                                    default_label,
                                ),
                            ));
                            return Ok(());
                        }
                        _ => {
                            app.push_display_message(DisplayMessage::error(
                                "Usage: /fast [on|off|status|default ...]",
                            ));
                            return Ok(());
                        }
                    };
                    remote.set_service_tier(service_tier).await?;
                    return Ok(());
                }

                if trimmed == "/transport" {
                    let current = app.remote_transport.as_deref().unwrap_or("unknown");
                    let transports = ["auto", "https", "websocket"];
                    let list: Vec<String> = transports
                        .iter()
                        .map(|t| {
                            if Some(*t) == app.remote_transport.as_deref() {
                                format!("**{}** ← current", t)
                            } else {
                                t.to_string()
                            }
                        })
                        .collect();
                    app.push_display_message(DisplayMessage::system(format!(
                        "Transport: {}\nAvailable: {}\nUse `/transport <mode>` to change.",
                        current,
                        list.join(" · ")
                    )));
                    return Ok(());
                }

                if let Some(mode) = trimmed.strip_prefix("/transport ") {
                    let mode = mode.trim();
                    if mode.is_empty() {
                        app.push_display_message(DisplayMessage::error("Usage: /transport <mode>"));
                        return Ok(());
                    }
                    remote.set_transport(mode).await?;
                    return Ok(());
                }

                if crate::tui::app::auth::handle_account_command_remote(app, trimmed, remote)
                    .await?
                {
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

                if super::commands::handle_goals_command(app, trimmed) {
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
                    let label = trimmed.strip_prefix("/save").unwrap_or_default().trim();
                    let label = if label.is_empty() {
                        None
                    } else {
                        Some(label.to_string())
                    };
                    if let Err(e) = persist_remote_session_metadata(app, |session| {
                        session.mark_saved(label.clone());
                    }) {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to save session: {}",
                            e
                        )));
                        return Ok(());
                    }
                    if app.memory_enabled {
                        if let Err(err) = remote.trigger_memory_extraction().await {
                            crate::logging::info(&format!(
                                "Failed to trigger memory extraction for saved remote session: {}",
                                err
                            ));
                        }
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
                    if let Err(e) = persist_remote_session_metadata(app, |session| {
                        session.unmark_saved();
                    }) {
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

                if trimmed == "/compact mode" || trimmed == "/compact mode status" {
                    let mode = app
                        .remote_compaction_mode
                        .clone()
                        .unwrap_or(crate::config::CompactionMode::Reactive);
                    app.push_display_message(DisplayMessage::system(format!(
                        "Compaction mode: **{}**\nAvailable: reactive · proactive · semantic\nUse `/compact mode <mode>` to change it for this session.",
                        mode.as_str()
                    )));
                    return Ok(());
                }

                if let Some(mode_str) = trimmed.strip_prefix("/compact mode ") {
                    let mode_str = mode_str.trim();
                    let Some(mode) = crate::config::CompactionMode::parse(mode_str) else {
                        app.push_display_message(DisplayMessage::error(
                            "Usage: `/compact mode <reactive|proactive|semantic>`".to_string(),
                        ));
                        return Ok(());
                    };
                    remote.set_compaction_mode(mode).await?;
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

                if trimmed == "/poke" {
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| app.session.id.clone());
                    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
                    let incomplete: Vec<_> = todos
                        .iter()
                        .filter(|t| t.status != "completed" && t.status != "cancelled")
                        .collect();

                    if incomplete.is_empty() {
                        app.push_display_message(DisplayMessage::system(
                            "No incomplete todos found. Nothing to poke about.".to_string(),
                        ));
                        return Ok(());
                    }

                    let mut todo_list = String::new();
                    for t in &incomplete {
                        let status_icon = match t.status.as_str() {
                            "in_progress" => "🔄",
                            _ => "⬜",
                        };
                        todo_list.push_str(&format!(
                            "  {} [{}] {}\n",
                            status_icon, t.priority, t.content
                        ));
                    }

                    let poke_msg = format!(
                        "Your todo list has {} incomplete item{}:\n\n{}\n\
                        Please continue your work. Either:\n\
                        1. Keep working and complete the remaining tasks\n\
                        2. Update the todo list with `todo_write` if items are already done or no longer needed\n\
                        3. If you genuinely need user input to proceed, say so clearly and specifically — \
                        but only if truly blocked (this should be rare; prefer making reasonable assumptions)",
                        incomplete.len(),
                        if incomplete.len() == 1 { "" } else { "s" },
                        todo_list,
                    );

                    if app.is_processing {
                        remote.cancel().await?;
                        app.set_status_notice("Interrupting for poke...");
                        app.push_display_message(DisplayMessage::system(format!(
                            "👉 Interrupting and poking with {} incomplete todo{}...",
                            incomplete.len(),
                            if incomplete.len() == 1 { "" } else { "s" },
                        )));
                        app.queued_messages.push(poke_msg);
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "👉 Poking model with {} incomplete todo{}...",
                            incomplete.len(),
                            if incomplete.len() == 1 { "" } else { "s" },
                        )));

                        let _ = super::remote::begin_remote_send(
                            app,
                            remote,
                            poke_msg,
                            vec![],
                            true,
                            None,
                            true,
                            0,
                        )
                        .await;
                    }
                    return Ok(());
                }

                if let Some(command) = super::commands::parse_improve_command(trimmed) {
                    match command {
                        Err(error) => app.push_display_message(DisplayMessage::error(error)),
                        Ok(super::commands::ImproveCommand::Resume) => {
                            let session_id = app
                                .remote_session_id
                                .clone()
                                .unwrap_or_else(|| app.session.id.clone());
                            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
                            let incomplete: Vec<_> = todos
                                .iter()
                                .filter(|todo| {
                                    todo.status != "completed" && todo.status != "cancelled"
                                })
                                .collect();

                            let mode = app.improve_mode.or_else(|| {
                                app.session
                                    .improve_mode
                                    .map(super::commands::restore_improve_mode)
                            });
                            let Some(mode) = mode else {
                                app.push_display_message(DisplayMessage::system(
                                    "No saved improve run found for this session. Use `/improve` or `/improve plan` to start one."
                                        .to_string(),
                                ));
                                return Ok(());
                            };

                            persist_remote_session_metadata(app, |session| {
                                session.improve_mode =
                                    Some(super::commands::session_improve_mode_for(mode));
                            })?;
                            app.improve_mode = Some(mode);
                            let prompt =
                                super::commands::build_improve_resume_prompt(mode, &incomplete);

                            if app.is_processing {
                                remote.cancel().await?;
                                app.set_status_notice("Interrupting for /improve resume...");
                                app.push_display_message(DisplayMessage::system(format!(
                                    "♻️ Interrupting and resuming {}...",
                                    mode.status_label()
                                )));
                                app.queued_messages.push(prompt);
                            } else {
                                app.push_display_message(DisplayMessage::system(format!(
                                    "♻️ Resuming {}...",
                                    mode.status_label()
                                )));
                                let _ = super::remote::begin_remote_send(
                                    app,
                                    remote,
                                    prompt,
                                    vec![],
                                    true,
                                    None,
                                    true,
                                    0,
                                )
                                .await;
                            }
                        }
                        Ok(super::commands::ImproveCommand::Status) => {
                            app.push_display_message(DisplayMessage::system(
                                super::commands::format_improve_status(app),
                            ));
                        }
                        Ok(super::commands::ImproveCommand::Stop) => {
                            let session_id = app
                                .remote_session_id
                                .clone()
                                .unwrap_or_else(|| app.session.id.clone());
                            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
                            let has_incomplete = todos.iter().any(|todo| {
                                todo.status != "completed" && todo.status != "cancelled"
                            });

                            if app.improve_mode.is_none() && !app.is_processing && !has_incomplete {
                                app.push_display_message(DisplayMessage::system(
                                    "No active improve loop to stop. Use `/improve` to start one."
                                        .to_string(),
                                ));
                                return Ok(());
                            }

                            persist_remote_session_metadata(app, |session| {
                                session.improve_mode = None;
                            })?;
                            app.improve_mode = None;
                            let stop_prompt = super::commands::improve_stop_prompt();
                            if app.is_processing {
                                remote.cancel().await?;
                                app.set_status_notice("Interrupting for /improve stop...");
                                app.push_display_message(DisplayMessage::system(
                                    super::commands::improve_stop_notice(true),
                                ));
                                app.queued_messages.push(stop_prompt);
                            } else {
                                app.push_display_message(DisplayMessage::system(
                                    super::commands::improve_stop_notice(false),
                                ));
                                let _ = super::remote::begin_remote_send(
                                    app,
                                    remote,
                                    stop_prompt,
                                    vec![],
                                    true,
                                    None,
                                    true,
                                    0,
                                )
                                .await;
                            }
                        }
                        Ok(super::commands::ImproveCommand::Run { plan_only, focus }) => {
                            let mode = super::commands::improve_mode_for(plan_only);
                            persist_remote_session_metadata(app, |session| {
                                session.improve_mode =
                                    Some(super::commands::session_improve_mode_for(mode));
                            })?;
                            app.improve_mode = Some(mode);
                            let prompt =
                                super::commands::build_improve_prompt(plan_only, focus.as_deref());
                            if app.is_processing {
                                remote.cancel().await?;
                                app.set_status_notice(if plan_only {
                                    "Interrupting for /improve plan..."
                                } else {
                                    "Interrupting for /improve..."
                                });
                                app.push_display_message(DisplayMessage::system(
                                    super::commands::improve_launch_notice(
                                        plan_only,
                                        focus.as_deref(),
                                        true,
                                    ),
                                ));
                                app.queued_messages.push(prompt);
                            } else {
                                app.push_display_message(DisplayMessage::system(
                                    super::commands::improve_launch_notice(
                                        plan_only,
                                        focus.as_deref(),
                                        false,
                                    ),
                                ));

                                let _ = super::remote::begin_remote_send(
                                    app,
                                    remote,
                                    prompt,
                                    vec![],
                                    true,
                                    None,
                                    true,
                                    0,
                                )
                                .await;
                            }
                        }
                    }
                    return Ok(());
                }

                if trimmed.starts_with('/') {
                    app.input = trimmed.to_string();
                    app.cursor_pos = app.input.len();
                    app.submit_input();
                    return Ok(());
                }

                if let Some(command) = input::extract_input_shell_command(&prepared.expanded) {
                    submit_remote_input_shell(app, remote, prepared.raw_input, command.to_string())
                        .await?;
                    return Ok(());
                }

                match app.send_action(false) {
                    SendAction::Submit => {
                        app.push_display_message(DisplayMessage {
                            role: "user".to_string(),
                            content: prepared.raw_input,
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                        let _ = app
                            .begin_remote_send(remote, prepared.expanded, prepared.images, false)
                            .await;
                    }
                    SendAction::Queue => {
                        app.queued_messages.push(prepared.expanded);
                    }
                    SendAction::Interleave => {
                        app.send_interleave_now(prepared.expanded, remote).await;
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
                input::clear_input_for_escape(app);
            } else if app.is_processing {
                remote.cancel().await?;
                app.set_status_notice("Interrupting...");
            } else {
                app.follow_chat_bottom();
                input::clear_input_for_escape(app);
            }
        }
        _ => {}
    }

    Ok(())
}
