use super::{
    App, DisplayMessage, ProcessingStatus, handle_terminal_event_while_disconnected,
    process_remote_followups,
};
use crate::tool::selfdev::ReloadContext;
use crate::tui::app::PendingReloadReconnectStatus;
use crate::tui::backend::{RemoteConnection, RemoteDisconnectReason};
use anyhow::Result;
use crossterm::event::EventStream;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::time::{Duration, Instant};
use tokio::time::MissedTickBehavior;

const RELOAD_MARKER_MAX_AGE: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(in crate::tui::app) struct RemoteRunState {
    pub reconnect_attempts: u32,
    pub disconnect_msg_idx: Option<usize>,
    pub disconnect_start: Option<Instant>,
    pub initial_server_start: bool,
    pub last_disconnect_reason: Option<String>,
    pub server_reload_in_progress: bool,
    pub reload_recovery_attempted: bool,
    pub last_reload_pid: Option<u32>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "connected outcome carries a live RemoteConnection while the small control variants remain simple"
)]
pub(in crate::tui::app) enum ConnectOutcome {
    Connected(RemoteConnection),
    Retry,
    Quit,
}

pub(in crate::tui::app) enum PostConnectOutcome {
    Ready,
    Quit,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::tui::app) struct ReloadReconnectHints {
    pub reload_ctx_for_session: Option<ReloadContext>,
    pub has_client_reload_marker: bool,
}

pub(super) fn format_disconnect_reason(reason: &RemoteDisconnectReason) -> String {
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

pub(in crate::tui::app) fn should_allow_reconnect_takeover(
    app: &App,
    state: &RemoteRunState,
    session_to_resume: Option<&str>,
) -> bool {
    if state.reconnect_attempts == 0 {
        return false;
    }

    let Some(session_to_resume) = session_to_resume else {
        return false;
    };

    app.remote_session_id
        .as_deref()
        .map(|remote_session_id| remote_session_id == session_to_resume)
        .unwrap_or(false)
}

pub(super) fn reconnect_status_message(app: &App, state: &RemoteRunState, detail: &str) -> String {
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
        format!(" · resume: jcode --resume {}", name)
    } else {
        String::new()
    };

    format!(
        "⚡ Connection lost — retrying (attempt {}, {}) — {}{}",
        state.reconnect_attempts.max(1),
        elapsed_str,
        detail,
        resume_hint,
    )
}

pub(super) fn reload_wait_status_message(
    app: &App,
    state: &RemoteRunState,
    detail: &str,
) -> String {
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
        format!(" · resume: jcode --resume {}", name)
    } else {
        String::new()
    };

    format!(
        "⚡ Server reload in progress — waiting for handoff ({}) — {}{}",
        elapsed_str, detail, resume_hint,
    )
}

fn set_disconnect_status_message(app: &mut App, state: &mut RemoteRunState, content: String) {
    if let Some(idx) = state.disconnect_msg_idx {
        let _ = app.replace_display_message_content(idx, content);
    } else {
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content,
            tool_calls: Vec::new(),
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        state.disconnect_msg_idx = Some(app.display_messages.len() - 1);
    }
}

fn disconnected_redraw_interval(initial_connect: bool) -> tokio::time::Interval {
    let period = if initial_connect {
        crate::tui::REDRAW_REMOTE_STARTUP
    } else {
        Duration::from_millis(1000)
    };
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval
}

pub(in crate::tui::app) fn reload_handoff_active(state: &RemoteRunState) -> bool {
    state.server_reload_in_progress || super::session_persistence::reload_marker_active()
}

pub(in crate::tui::app) fn should_use_same_session_fast_path(
    reconnected_after_disconnect: bool,
    session_to_resume: Option<&str>,
    remote_session_id: Option<&str>,
    has_display_messages: bool,
    reload_reconnect_needs_server_history: bool,
) -> bool {
    reconnected_after_disconnect
        && !reload_reconnect_needs_server_history
        && session_to_resume
            .zip(remote_session_id)
            .map(|(resume_id, remote_id)| resume_id == remote_id)
            .unwrap_or(false)
        && has_display_messages
}

async fn wait_for_reload_handoff_before_reconnect(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event_stream: &mut EventStream,
    state: &mut RemoteRunState,
) -> Result<Option<ConnectOutcome>> {
    if !reload_handoff_active(state) {
        return Ok(None);
    }

    state.disconnect_start.get_or_insert_with(Instant::now);
    app.set_remote_startup_phase(super::super::RemoteStartupPhase::WaitingForReload);
    app.set_status_notice("Waiting for reload handoff...");
    let detail = state
        .last_disconnect_reason
        .as_deref()
        .unwrap_or("server reload in progress");
    set_disconnect_status_message(app, state, reload_wait_status_message(app, state, detail));
    terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;

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
                "Reconnect reload handoff: ready before next connect attempt (state={})",
                crate::server::reload_state_summary(RELOAD_MARKER_MAX_AGE)
            ));
            Ok(None)
        }
        crate::server::ReloadWaitStatus::Failed(detail) => {
            crate::logging::warn(&format!(
                "Reconnect reload handoff pre-check: failed detail={:?} state={}",
                detail,
                crate::server::reload_state_summary(RELOAD_MARKER_MAX_AGE)
            ));
            let detail = detail.unwrap_or_else(|| {
                "reload failed before the replacement server became ready; starting replacement server"
                    .to_string()
            });
            if recover_reloading_server(app, terminal, state, &detail).await? {
                Ok(Some(ConnectOutcome::Retry))
            } else {
                Ok(None)
            }
        }
        crate::server::ReloadWaitStatus::Idle => {
            crate::logging::warn(&format!(
                "Reconnect reload handoff pre-check: idle without ready server state={}",
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
                Ok(Some(ConnectOutcome::Retry))
            } else {
                Ok(None)
            }
        }
        crate::server::ReloadWaitStatus::Waiting { pid } => {
            state.last_reload_pid = pid;
            crate::logging::info(&format!(
                "Reconnect wait: pausing reconnect attempts for reload lifecycle event (pid={:?}, state={})",
                pid,
                crate::server::reload_state_summary(RELOAD_MARKER_MAX_AGE)
            ));
            let wait = crate::server::wait_for_reload_handoff_event(pid, &socket_path);
            tokio::pin!(wait);
            let mut redraw = disconnected_redraw_interval(false);
            loop {
                tokio::select! {
                    _ = &mut wait => break,
                    _ = redraw.tick() => {
                        terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
                    }
                    event = event_stream.next() => {
                        if handle_terminal_event_while_disconnected(app, terminal, event)? {
                            return Ok(Some(ConnectOutcome::Quit));
                        }
                    }
                }
            }
            Ok(Some(ConnectOutcome::Retry))
        }
    }
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

    match crate::cli::dispatch::spawn_server(
        &crate::cli::provider_init::ProviderChoice::Auto,
        None,
        None,
    )
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

pub(in crate::tui::app) async fn connect_with_retry(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event_stream: &mut EventStream,
    state: &mut RemoteRunState,
    session_to_resume: Option<&str>,
) -> Result<ConnectOutcome> {
    if let Some(outcome) =
        wait_for_reload_handoff_before_reconnect(app, terminal, event_stream, state).await?
    {
        return Ok(outcome);
    }

    let client_has_local_history =
        session_to_resume.is_some() && !app.display_messages().is_empty();
    let client_instance_id = app.remote_client_instance_id.clone();
    let allow_session_takeover = should_allow_reconnect_takeover(app, state, session_to_resume);
    let connect = RemoteConnection::connect_with_session(
        session_to_resume,
        Some(client_instance_id.as_str()),
        client_has_local_history,
        allow_session_takeover,
    );
    crate::logging::info(&format!(
        "Remote reconnect attempt: resume={:?}, reconnect_attempts={}, client_instance_id={}, local_history={}, allow_takeover={}",
        session_to_resume,
        state.reconnect_attempts,
        client_instance_id,
        client_has_local_history,
        allow_session_takeover,
    ));
    tokio::pin!(connect);
    let mut redraw = disconnected_redraw_interval(state.reconnect_attempts == 0);

    match loop {
        tokio::select! {
            result = &mut connect => break result,
            _ = redraw.tick() => {
                terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
            }
            event = event_stream.next() => {
                if handle_terminal_event_while_disconnected(app, terminal, event)? {
                    return Ok(ConnectOutcome::Quit);
                }
            }
        }
    } {
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

            let msg_content = if is_initial_server_start || state.initial_server_start {
                app.set_remote_startup_phase(super::super::RemoteStartupPhase::StartingServer);
                "⏳ Starting server...".to_string()
            } else {
                app.set_remote_startup_phase(super::super::RemoteStartupPhase::Reconnecting {
                    attempt: state.reconnect_attempts,
                });
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

            set_disconnect_status_message(app, state, msg_content);
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
                        let mut redraw = disconnected_redraw_interval(false);
                        loop {
                            tokio::select! {
                                _ = &mut wait => break,
                                _ = redraw.tick() => {
                                    terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
                                }
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

            let backoff = if (state.initial_server_start && state.reconnect_attempts <= 20)
                || state.reconnect_attempts <= 2
            {
                Duration::from_millis(100)
            } else {
                if state.initial_server_start {
                    state.initial_server_start = false;
                }
                Duration::from_secs((1u64 << (state.reconnect_attempts - 2).min(5)).min(30))
            };
            let sleep = tokio::time::sleep(backoff);
            tokio::pin!(sleep);
            let mut redraw = disconnected_redraw_interval(false);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    _ = redraw.tick() => {
                        terminal.draw(|frame| crate::tui::ui::draw(frame, app))?;
                    }
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

            Ok(ConnectOutcome::Retry)
        }
    }
}

pub(in crate::tui::app) async fn handle_post_connect<B: ratatui::backend::Backend>(
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
    let has_reload_ctx_for_session = hints.reload_ctx_for_session.is_some();
    if state.reconnect_attempts > 0 {
        if let Some(disconnect_start) = state.disconnect_start {
            crate::logging::info(&format!(
                "Reload reconnect succeeded after {}ms (attempts={})",
                disconnect_start.elapsed().as_millis(),
                state.reconnect_attempts
            ));
        }
        if app.reload_info.is_empty()
            && let Some(ctx) = hints.reload_ctx_for_session.as_ref()
        {
            app.reload_info.push(ctx.reconnect_notice_line());
        }

        let must_reload_client = state.server_reload_in_progress || app.has_newer_binary();

        if must_reload_client {
            app.push_display_message(DisplayMessage::system(
                "Server reloaded. Reloading client binary...".to_string(),
            ));
            terminal
                .draw(|frame| crate::tui::ui::draw(frame, app))
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let session_id = app
                .remote_session_id
                .clone()
                .unwrap_or_else(|| crate::id::new_id("ses"));
            if (has_reload_ctx_for_session || !app.reload_info.is_empty())
                && let Ok(jcode_dir) = crate::storage::jcode_dir()
            {
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

    let reload_ctx_available = hints.reload_ctx_for_session.is_some();
    let history_already_loaded = remote.has_loaded_history();
    let reload_reconnect_needs_server_history = state.server_reload_in_progress
        && !reload_ctx_available
        && !hints.has_client_reload_marker
        && !history_already_loaded;
    let same_session_reload_fast_path = should_use_same_session_fast_path(
        state.reconnect_attempts > 0,
        session_to_resume,
        app.remote_session_id.as_deref(),
        !app.display_messages.is_empty(),
        reload_reconnect_needs_server_history,
    );

    if reload_reconnect_needs_server_history {
        app.pending_reload_reconnect_status = Some(PendingReloadReconnectStatus::AwaitingHistory {
            session_id: session_to_resume.map(str::to_string),
        });
        app.push_display_message(DisplayMessage::system(
            "Reload complete — checking restored history to decide whether continuation is needed."
                .to_string(),
        ));
        ReloadContext::log_recovery_outcome(
            "tui_reconnect",
            session_to_resume.unwrap_or("unknown"),
            "deferred",
            "reload reconnect without local reload context; waiting for server history payload to determine continuation",
        );
    }

    if same_session_reload_fast_path
        || hints.has_client_reload_marker
        || (reload_ctx_available && history_already_loaded)
    {
        finalize_reload_reconnect(app, session_to_resume, hints, state.reconnect_attempts > 0);
    } else if reload_ctx_available {
        ReloadContext::log_recovery_outcome(
            "tui_reconnect",
            session_to_resume.unwrap_or("unknown"),
            "deferred",
            "waiting for server history payload to deliver reload recovery directive",
        );
        app.reload_info.clear();
    }

    state.reconnect_attempts = 0;
    state.initial_server_start = false;
    state.server_reload_in_progress = false;

    if same_session_reload_fast_path {
        crate::logging::info(
            "Same-session reload fast path: skipping blocking History wait and reusing local display state",
        );
        remote.mark_history_loaded();
        app.clear_remote_startup_phase();
    } else if !remote.has_loaded_history() {
        app.set_remote_startup_phase(super::super::RemoteStartupPhase::LoadingSession);
    } else {
        app.clear_remote_startup_phase();
    }

    if remote.has_loaded_history() && !app.is_processing && app.has_queued_followups() {
        crate::logging::info(
            "Post-connect history restored with queued followups; dispatching immediately",
        );
        if app.pending_queued_dispatch {
            crate::logging::info(
                "Clearing deferred queued-dispatch flag during post-connect so restored followups continue immediately",
            );
            app.pending_queued_dispatch = false;
        }
        process_remote_followups(app, remote).await;
    }

    Ok(PostConnectOutcome::Ready)
}

pub(super) fn load_reload_reconnect_hints(
    app: &mut App,
    session_to_resume: Option<&str>,
) -> ReloadReconnectHints {
    let reload_ctx_for_session = session_to_resume.and_then(|sid| {
        let result = ReloadContext::peek_for_session(sid);
        crate::logging::info(&format!(
            "Reload peek_for_session({}) = {:?}",
            sid,
            result.as_ref().map(|r| r.is_some())
        ));
        result.ok().flatten()
    });

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
        reload_ctx_for_session,
        has_client_reload_marker,
    }
}

pub(in crate::tui::app) fn finalize_reload_reconnect(
    app: &mut App,
    session_to_resume: Option<&str>,
    hints: ReloadReconnectHints,
    reconnected_after_disconnect: bool,
) {
    let should_queue_reload_continuation = hints.reload_ctx_for_session.is_some();
    crate::logging::info(&format!(
        "Reload continuation check: should_queue={}, reload_info_empty={}, has_ctx={}, has_marker={}",
        should_queue_reload_continuation,
        app.reload_info.is_empty(),
        hints.reload_ctx_for_session.is_some(),
        hints.has_client_reload_marker
    ));
    if should_queue_reload_continuation {
        app.pending_reload_reconnect_status = None;
        let reload_ctx = session_to_resume.and_then(|sid| {
            let result = ReloadContext::load_for_session(sid);
            crate::logging::info(&format!(
                "Reload load_for_session({}) = {:?}",
                sid,
                result.as_ref().map(|r| r.is_some())
            ));
            result.ok().flatten()
        });

        let background_task_note = session_to_resume
            .map(super::super::reload_persisted_background_tasks_note)
            .unwrap_or_default();
        let directive = ReloadContext::recovery_directive(
            reload_ctx.as_ref(),
            false,
            &background_task_note,
            None,
        );

        if let Some(directive) = directive {
            let session_id = session_to_resume.unwrap_or("unknown");
            if app.current_message_id.is_none()
                && (app.remote_resume_activity.is_some() || app.is_processing)
            {
                crate::logging::info(
                    "Reload reconnect: clearing stale resumed-processing state before dispatching continuation",
                );
                app.remote_resume_activity = None;
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.processing_started = None;
                app.clear_visible_turn_started();
                app.last_stream_activity = None;
                app.replay_processing_started_ms = None;
                app.replay_elapsed_override = None;
            }

            crate::logging::info(&format!(
                "Queuing reload continuation message ({} chars)",
                directive.continuation_message.len()
            ));
            ReloadContext::log_recovery_outcome(
                "tui_reconnect",
                session_id,
                "resumed",
                "queued initiator continuation after reconnect",
            );
            app.push_display_message(DisplayMessage::system(
                "Reload complete — continuing because reload recovery was pending.",
            ));
            app.hidden_queued_system_messages
                .push(directive.continuation_message);
        } else {
            ReloadContext::log_recovery_outcome(
                "tui_reconnect",
                session_to_resume.unwrap_or("unknown"),
                "failed",
                "reload context missing for reconnecting initiator session",
            );
            crate::logging::warn(
                "Reload context missing for initiating session after reconnect; skipping selfdev continuation",
            );
        }
        app.reload_info.clear();
    } else if hints.has_client_reload_marker {
        app.pending_reload_reconnect_status = None;
        ReloadContext::log_recovery_outcome(
            "tui_reconnect",
            session_to_resume.unwrap_or("unknown"),
            "skipped",
            "client reload marker present without session reload context",
        );
        if !reconnected_after_disconnect && !app.reload_info.is_empty() {
            app.push_display_message(DisplayMessage::system(app.reload_info.join("\n")));
        }
        app.push_display_message(DisplayMessage::system(
            "Reload complete — no continuation queued because no recovery context was pending."
                .to_string(),
        ));
        app.reload_info.clear();
    }
}
