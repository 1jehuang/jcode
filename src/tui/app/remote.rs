#![cfg_attr(test, allow(clippy::items_after_test_module))]

use super::{
    App, DisplayMessage, ProcessingStatus, RemoteResumeActivity, SendAction,
    ctrl_bracket_fallback_to_esc, input, parse_rate_limit_error,
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

mod reconnect;
mod workspace;

#[allow(unused_imports)]
pub(super) use reconnect::{
    ConnectOutcome, PostConnectOutcome, ReloadReconnectHints, RemoteRunState, connect_with_retry,
    finalize_reload_reconnect, handle_post_connect, reload_handoff_active,
    should_allow_reconnect_takeover,
};
use reconnect::{format_disconnect_reason, reconnect_status_message};
use workspace::{handle_workspace_command, handle_workspace_navigation_key};

const CONNECTION_MESSAGE_TITLE: &str = "Connection";
const RELOAD_MARKER_MAX_AGE: Duration = Duration::from_secs(30);

impl App {
    fn track_pending_soft_interrupt(&mut self, request_id: u64, content: String) {
        self.pending_soft_interrupt_requests
            .push((request_id, content.clone()));
        self.pending_soft_interrupts.push(content);
    }

    fn acknowledge_pending_soft_interrupt(&mut self, request_id: u64) -> bool {
        if let Some(index) = self
            .pending_soft_interrupt_requests
            .iter()
            .position(|(id, _)| *id == request_id)
        {
            self.pending_soft_interrupt_requests.remove(index);
            true
        } else {
            false
        }
    }

    fn clear_pending_soft_interrupt_tracking(&mut self) {
        self.pending_soft_interrupts.clear();
        self.pending_soft_interrupt_requests.clear();
    }

    fn mark_soft_interrupt_injected(&mut self, content: &str) {
        if let Some(index) = self
            .pending_soft_interrupts
            .iter()
            .position(|pending| pending == content)
        {
            self.pending_soft_interrupts.remove(index);
        }

        if let Some(index) = self
            .pending_soft_interrupt_requests
            .iter()
            .position(|(_, pending)| pending == content)
        {
            self.pending_soft_interrupt_requests.remove(index);
        }
    }
}

fn recover_local_interleave_to_queue(app: &mut App, reason: &str) -> bool {
    let Some(interleave) = app.interleave_message.take() else {
        return false;
    };
    if interleave.trim().is_empty() {
        return false;
    }

    crate::logging::info(&format!(
        "Recovering unsent interleave into queued follow-ups after {}",
        reason
    ));
    app.queued_messages.insert(0, interleave);
    true
}

async fn recover_stranded_soft_interrupts(app: &mut App, remote: &mut RemoteConnection) -> bool {
    if app.is_processing || app.pending_soft_interrupts.is_empty() {
        return false;
    }

    let recovered_interrupts = std::mem::take(&mut app.pending_soft_interrupts);
    if recovered_interrupts.is_empty() {
        return false;
    }

    if let Err(err) = remote.cancel_soft_interrupts().await {
        app.pending_soft_interrupts = recovered_interrupts;
        app.push_display_message(DisplayMessage::error(format!(
            "Failed to recover queued interleave message: {}",
            err
        )));
        app.set_status_notice("Queued interleave recovery failed");
        return false;
    }

    crate::logging::info(&format!(
        "Recovering {} stranded soft interrupt(s) into queued follow-ups after turn boundary",
        recovered_interrupts.len()
    ));
    app.pending_soft_interrupt_requests.clear();

    let mut recovered_queue = recovered_interrupts;
    recovered_queue.append(&mut app.queued_messages);
    app.queued_messages = recovered_queue;
    app.set_status_notice("Recovered queued interleave after turn finished");
    true
}

pub(super) enum RemoteEventOutcome {
    Continue,
    Reconnect,
}

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

pub(super) async fn handle_tick(app: &mut App, remote: &mut RemoteConnection) -> bool {
    let mut needs_redraw = crate::tui::periodic_redraw_required(app);
    app.progress_mouse_scroll_animation();
    if let Some(chunk) = app.stream_buffer.flush() {
        app.streaming_text.push_str(&chunk);
        needs_redraw = true;
    }

    needs_redraw |= app.refresh_side_panel_linked_content_if_due();

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
    Ok(needs_redraw)
}

#[cfg(test)]
mod tests {
    use super::reconnect;
    use super::{RemoteRunState, handle_post_connect, handle_server_event, process_remote_followups};
    use crate::protocol::{
        MemoryActivitySnapshot, MemoryPipelineSnapshot, MemoryStateSnapshot,
        MemoryStepStatusSnapshot, ServerEvent,
    };
    use crate::provider::Provider;
    use crate::tui::info_widget::{MemoryState, StepStatus};
    use anyhow::Result;
    use std::sync::Arc;

    struct MockProvider;

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            unimplemented!("Mock provider")
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self)
        }
    }

    fn create_test_app() -> crate::tui::app::App {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
        let mut app = crate::tui::app::App::new(provider, registry);
        app.queue_mode = false;
        app.diff_mode = crate::config::DiffDisplayMode::Inline;
        app
    }

    #[test]
    fn reload_handoff_active_when_server_flag_is_set() {
        let state = RemoteRunState {
            server_reload_in_progress: true,
            ..RemoteRunState::default()
        };

        assert!(reconnect::reload_handoff_active(&state));
    }

    #[test]
    fn reload_handoff_inactive_without_flag_or_marker() {
        assert!(!reconnect::reload_handoff_active(&RemoteRunState::default()));
    }

    #[test]
    fn reload_wait_status_message_uses_waiting_language() {
        let mut app = create_test_app();
        app.resume_session_id = Some("ses_test_reload_wait".to_string());
        let state = RemoteRunState::default();

        let message =
            reconnect::reload_wait_status_message(&app, &state, "server reload in progress");

        assert!(message.contains("waiting for handoff"));
        assert!(!message.contains("retrying"));
    }

    #[test]
    fn process_remote_followups_auto_reloads_server_by_default() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();

        app.pending_server_reload = true;
        app.auto_server_reload = true;

        rt.block_on(process_remote_followups(&mut app, &mut remote));

        assert!(!app.pending_server_reload);
        let last = app
            .display_messages()
            .last()
            .expect("missing reload message");
        assert_eq!(last.title.as_deref(), Some("Reload"));
        assert!(last.content.contains("Reloading server with newer binary"));
    }

    #[test]
    fn process_remote_followups_respects_disabled_auto_server_reload() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();

        app.pending_server_reload = true;
        app.auto_server_reload = false;

        rt.block_on(process_remote_followups(&mut app, &mut remote));

        assert!(!app.pending_server_reload);
        let last = app.display_messages().last().expect("missing info message");
        assert_eq!(last.role, "system");
        assert!(last.content.contains("display.auto_server_reload = false"));
    }

    #[test]
    fn handle_post_connect_dispatches_reload_followup_even_if_history_snapshot_looks_busy() {
        let _guard = crate::storage::lock_test_env();
        let temp_home = tempfile::TempDir::new().expect("create temp home");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp_home.path());

        let session_id = "session_reload_busy_snapshot";
        crate::tool::selfdev::ReloadContext {
            task_context: Some("Validate reload continuation after reconnect".to_string()),
            version_before: "old-build".to_string(),
            version_after: "new-build".to_string(),
            session_id: session_id.to_string(),
            timestamp: "2026-04-14T00:00:00Z".to_string(),
        }
        .save()
        .expect("save reload context");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let mut app = crate::tui::app::App::new_for_remote(Some(session_id.to_string()));
        app.queue_mode = false;
        app.diff_mode = crate::config::DiffDisplayMode::Inline;
        app.is_processing = true;
        app.status = crate::tui::app::ProcessingStatus::RunningTool("batch".to_string());
        app.processing_started = Some(std::time::Instant::now());
        app.remote_resume_activity = Some(crate::tui::app::RemoteResumeActivity {
            session_id: session_id.to_string(),
            observed_at: std::time::Instant::now(),
            current_tool_name: Some("batch".to_string()),
        });

        let _enter = rt.enter();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut state = super::RemoteRunState {
            reconnect_attempts: 1,
            ..Default::default()
        };

        let outcome = rt
            .block_on(handle_post_connect(
                &mut app,
                &mut terminal,
                &mut remote,
                &mut state,
                Some(session_id),
            ))
            .expect("post connect should succeed");

        assert!(matches!(outcome, super::PostConnectOutcome::Ready));
        assert!(
            app.hidden_queued_system_messages.is_empty(),
            "reload continuation should dispatch instead of staying hidden"
        );
        assert!(matches!(app.status, crate::tui::app::ProcessingStatus::Sending));
        assert!(app.current_message_id.is_some());
        assert!(app.rate_limit_pending_message.is_some());

        if let Ok(path) = crate::tool::selfdev::ReloadContext::path_for_session(session_id) {
            let _ = std::fs::remove_file(path);
        }
        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn handle_server_event_applies_remote_memory_activity_snapshot() {
        crate::memory::clear_activity();

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.memory_enabled = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        handle_server_event(
            &mut app,
            ServerEvent::MemoryActivity {
                activity: MemoryActivitySnapshot {
                    state: MemoryStateSnapshot::SidecarChecking { count: 3 },
                    state_age_ms: 180,
                    pipeline: Some(MemoryPipelineSnapshot {
                        search: MemoryStepStatusSnapshot::Done,
                        search_result: None,
                        verify: MemoryStepStatusSnapshot::Running,
                        verify_result: None,
                        verify_progress: Some((1, 3)),
                        inject: MemoryStepStatusSnapshot::Pending,
                        inject_result: None,
                        maintain: MemoryStepStatusSnapshot::Pending,
                        maintain_result: None,
                    }),
                },
            },
            &mut remote,
        );

        let activity = crate::memory::get_activity().expect("memory activity should be populated");
        assert_eq!(activity.state, MemoryState::SidecarChecking { count: 3 });
        let pipeline = activity.pipeline.expect("pipeline should be restored");
        assert_eq!(pipeline.search, StepStatus::Done);
        assert_eq!(pipeline.verify, StepStatus::Running);
        assert_eq!(pipeline.verify_progress, Some((1, 3)));
        assert!(activity.state_since.elapsed().as_millis() >= 100);

        crate::memory::clear_activity();
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
        Ok(BusEvent::SessionUpdateStatus(status)) => {
            app.handle_session_update_status(status);
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
    if recovered_local || !app.pending_soft_interrupts.is_empty() {
        crate::logging::info(&format!(
            "Preserving {} pending soft interrupt(s) across disconnect",
            app.pending_soft_interrupts.len()
        ));
    }
    app.reset_streaming_tps();
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

#[expect(
    clippy::too_many_arguments,
    reason = "remote send needs explicit message payload, reminders, retry metadata, and image attachments"
)]
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
    app.status_detail = None;
    app.processing_started = Some(Instant::now());
    app.last_stream_activity = Some(Instant::now());
    app.remote_resume_activity = None;
    app.reset_streaming_tps();
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
    app.autoreview_after_current_turn = !is_system;
    app.autojudge_after_current_turn = !is_system;
    remote.reset_call_output_tokens_seen();
    Ok(msg_id)
}

fn restore_prepared_remote_input(app: &mut App, prepared: input::PreparedInput) {
    app.input = prepared.raw_input;
    app.cursor_pos = app.input.len();
    app.pending_images = prepared.images;
}

fn history_matches_pending_startup_prompt(app: &App) -> bool {
    if !app.submit_input_on_startup || !app.pending_images.is_empty() || app.input.trim().is_empty()
    {
        return false;
    }

    app.display_messages()
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .is_some_and(|message| message.content == app.input)
}

async fn submit_prepared_remote_input(
    app: &mut App,
    remote: &mut RemoteConnection,
    prepared: input::PreparedInput,
) -> Result<()> {
    if let Some(command) = input::extract_input_shell_command(&prepared.expanded) {
        submit_remote_input_shell(app, remote, prepared.raw_input, command.to_string()).await?;
        return Ok(());
    }

    app.commit_pending_streaming_assistant_message();
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
    Ok(())
}

async fn route_prepared_input_to_new_remote_session(
    app: &mut App,
    remote: &mut RemoteConnection,
    prepared: input::PreparedInput,
) -> Result<()> {
    app.route_next_prompt_to_new_session = false;
    app.pending_split_startup_message = None;
    app.pending_split_prompt = Some(super::PendingSplitPrompt {
        content: prepared.expanded,
        images: prepared.images,
    });
    app.pending_split_model_override = None;
    app.pending_split_provider_key_override = None;
    app.pending_split_label = Some("Prompt".to_string());
    app.pending_split_started_at = Some(Instant::now());

    if app.is_processing {
        app.pending_split_request = true;
        app.set_status_notice("Prompt queued for new session");
        return Ok(());
    }

    app.pending_split_request = false;
    begin_remote_split_launch(app, "Prompt");
    if let Err(error) = remote.split().await {
        finish_remote_split_launch(app);
        let pending = app
            .pending_split_prompt
            .take()
            .map(|prompt| input::PreparedInput {
                raw_input: prepared.raw_input,
                expanded: prompt.content,
                images: prompt.images,
            });
        app.pending_split_model_override = None;
        app.pending_split_provider_key_override = None;
        app.pending_split_label = None;
        if let Some(prepared) = pending {
            restore_prepared_remote_input(app, prepared);
        }
        return Err(error);
    }
    Ok(())
}

fn begin_remote_split_launch(app: &mut App, label: &str) {
    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.status_detail = None;
    let started_at = Instant::now();
    app.pending_split_started_at = Some(started_at);
    app.processing_started = Some(started_at);
    app.last_stream_activity = Some(started_at);
    app.remote_resume_activity = None;
    app.reset_streaming_tps();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.current_message_id = None;
    app.set_status_notice(format!("{} launching", label));
}

fn finish_remote_split_launch(app: &mut App) {
    if !app.is_processing || app.current_message_id.is_some() {
        return;
    }
    if !matches!(app.status, ProcessingStatus::Sending) {
        return;
    }
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.processing_started = None;
    app.last_stream_activity = None;
    app.reset_streaming_tps();
    app.current_message_id = None;
}

fn set_transcript_input(app: &mut App, text: String) {
    app.input = text;
    app.cursor_pos = app.input.len();
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
}

fn transcript_send_text(text: &str) -> String {
    const TRANSCRIPTION_PREFIX: &str = "[transcription]";

    let trimmed_start = text.trim_start();
    if trimmed_start.is_empty()
        || trimmed_start.starts_with(TRANSCRIPTION_PREFIX)
        || trimmed_start.starts_with('/')
        || trimmed_start.starts_with('!')
    {
        return text.to_string();
    }

    format!("{} {}", TRANSCRIPTION_PREFIX, trimmed_start)
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

async fn submit_remote_transcript_input(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Result<()> {
    let trimmed = app.input.trim().to_string();
    if trimmed.is_empty() {
        app.set_status_notice("Transcript was empty");
        return Ok(());
    }

    if trimmed.starts_with('/') {
        app.submit_input();
        return Ok(());
    }

    if let Some(command) = input::extract_input_shell_command(&trimmed) {
        let raw_input = std::mem::take(&mut app.input);
        app.cursor_pos = 0;
        app.clear_input_undo_history();
        submit_remote_input_shell(app, remote, raw_input, command.to_string()).await?;
        return Ok(());
    }

    match app.send_action(false) {
        SendAction::Submit => {
            let prepared = input::take_prepared_input(app);
            app.push_display_message(DisplayMessage {
                role: "user".to_string(),
                content: prepared.raw_input,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            app.begin_remote_send(remote, prepared.expanded, prepared.images, false)
                .await?;
        }
        SendAction::Queue => queue_transcript_input(app),
        SendAction::Interleave => {
            let prepared = input::take_prepared_input(app);
            app.send_interleave_now(prepared.expanded, remote).await;
        }
    }

    Ok(())
}

async fn submit_remote_input_shell(
    app: &mut App,
    remote: &mut RemoteConnection,
    raw_input: String,
    command: String,
) -> Result<()> {
    app.commit_pending_streaming_assistant_message();
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
    app.status_detail = None;
    app.processing_started = Some(Instant::now());
    app.last_stream_activity = Some(Instant::now());
    app.remote_resume_activity = None;
    app.reset_streaming_tps();
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
            let text = transcript_send_text(&text);
            input::insert_input_text(app, &text);
            submit_transcript_input(app);
        }
    }

    app.follow_chat_bottom_for_typing();
}

pub(super) async fn apply_remote_transcript_event(
    app: &mut App,
    remote: &mut RemoteConnection,
    text: String,
    mode: TranscriptMode,
) -> Result<()> {
    if text.trim().is_empty() {
        app.set_status_notice("Transcript was empty");
        return Ok(());
    }

    match mode {
        TranscriptMode::Send => {
            let text = transcript_send_text(&text);
            input::insert_input_text(app, &text);
            submit_remote_transcript_input(app, remote).await?;
        }
        _ => apply_transcript_event(app, text, mode),
    }

    app.follow_chat_bottom_for_typing();
    Ok(())
}

pub(super) fn handle_server_event(
    app: &mut App,
    event: ServerEvent,
    remote: &mut impl RemoteEventState,
) -> bool {
    let eager_stream_redraw = !crate::perf::tui_policy().enable_decorative_animations;
    if app.is_processing {
        app.last_stream_activity = Some(Instant::now());
    }

    if matches!(
        &event,
        ServerEvent::TextDelta { .. }
            | ServerEvent::TextReplace { .. }
            | ServerEvent::ToolStart { .. }
            | ServerEvent::ToolInput { .. }
            | ServerEvent::ToolExec { .. }
            | ServerEvent::ToolDone { .. }
            | ServerEvent::BatchProgress { .. }
            | ServerEvent::TokenUsage { .. }
            | ServerEvent::ConnectionType { .. }
            | ServerEvent::ConnectionPhase { .. }
            | ServerEvent::StatusDetail { .. }
            | ServerEvent::UpstreamProvider { .. }
            | ServerEvent::Interrupted
            | ServerEvent::Done { .. }
            | ServerEvent::Error { .. }
    ) {
        app.remote_resume_activity = None;
    }

    let call_output_tokens_seen = remote.call_output_tokens_seen();

    match event {
        ServerEvent::TextDelta { text } => {
            if let Some(thought_line) = App::extract_thought_line(&text) {
                if let Some(chunk) = app.stream_buffer.flush() {
                    app.streaming_text.push_str(&chunk);
                }
                app.insert_thought_line(thought_line);
                return eager_stream_redraw;
            }
            let mut needs_redraw = false;
            if matches!(
                app.status,
                ProcessingStatus::Sending
                    | ProcessingStatus::Connecting(_)
                    | ProcessingStatus::Thinking(_)
            ) || (app.is_processing && matches!(app.status, ProcessingStatus::Idle))
            {
                app.status = ProcessingStatus::Streaming;
                needs_redraw = true;
            }
            app.resume_streaming_tps();
            if let Some(chunk) = app.stream_buffer.push(&text) {
                app.streaming_text.push_str(&chunk);
                needs_redraw = true;
            }
            app.last_stream_activity = Some(Instant::now());
            eager_stream_redraw && needs_redraw
        }
        ServerEvent::TextReplace { text } => {
            app.stream_buffer.flush();
            app.streaming_text = text;
            app.resume_streaming_tps();
            true
        }
        ServerEvent::ToolStart { id, name } => {
            app.pause_streaming_tps(false);
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
            eager_stream_redraw
        }
        ServerEvent::ToolInput { delta } => {
            remote.handle_tool_input(&delta);
            false
        }
        ServerEvent::ToolExec { id, name } => {
            app.pause_streaming_tps(false);
            let parsed_input = remote.get_current_tool_input();
            let tool_call = ToolCall {
                id: id.clone(),
                name: name.clone(),
                input: parsed_input.clone(),
                intent: None,
            };
            if let Some(tc) = app.streaming_tool_calls.iter_mut().find(|tc| tc.id == id) {
                tc.input = parsed_input;
            }
            remote.handle_tool_exec(&id, &name);
            app.observe_tool_call(&tool_call);
            eager_stream_redraw
                || app.side_panel.focused_page_id.as_deref()
                    == Some(super::observe::OBSERVE_PAGE_ID)
        }
        ServerEvent::ToolDone {
            id,
            name,
            output,
            error,
        } => {
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
            let tool_input = app
                .streaming_tool_calls
                .iter()
                .find(|tc| tc.id == id)
                .map(|tc| tc.input.clone())
                .unwrap_or(serde_json::Value::Null);
            let tool_call = ToolCall {
                id,
                name,
                input: tool_input,
                intent: None,
            };
            app.commit_pending_streaming_assistant_message();
            crate::tui::mermaid::clear_streaming_preview_diagram();
            let is_batch = tool_call.name == "batch";
            app.observe_tool_result(&tool_call, &output, error.is_some(), None);
            app.push_display_message(DisplayMessage {
                role: "tool".to_string(),
                content: display_output,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: Some(tool_call),
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
            eager_stream_redraw && matches!(app.status, ProcessingStatus::Streaming)
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
            app.status = if matches!(cp, crate::message::ConnectionPhase::Streaming) {
                ProcessingStatus::Streaming
            } else {
                ProcessingStatus::Connecting(cp)
            };
            eager_stream_redraw
        }
        ServerEvent::StatusDetail { detail } => {
            app.status_detail = Some(detail);
            eager_stream_redraw
        }
        ServerEvent::UpstreamProvider { provider } => {
            app.upstream_provider = Some(provider);
            false
        }
        ServerEvent::Ack { id } => {
            let _ = app.acknowledge_pending_soft_interrupt(id);
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
            let recovered_local = recover_local_interleave_to_queue(app, "interrupt");
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
            if recovered_local || !app.pending_soft_interrupts.is_empty() {
                crate::logging::info(&format!(
                    "Preserving {} pending soft interrupt(s) across interrupt",
                    app.pending_soft_interrupts.len()
                ));
            }
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
                app.pause_streaming_tps(false);
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
                    crate::logging::info(&format!(
                        "Ignoring unrelated Done id={} while processing current_message_id={:?}; preserving active/queued turn",
                        id, app.current_message_id
                    ));
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
            let is_failover_prompt =
                crate::provider::parse_failover_prompt_message(&message).is_some();
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
            let recovered_local = recover_local_interleave_to_queue(app, "request error");
            crate::tui::mermaid::clear_streaming_preview_diagram();
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            if recovered_local || !app.pending_soft_interrupts.is_empty() {
                crate::logging::info(&format!(
                    "Preserving {} pending soft interrupt(s) across remote error",
                    app.pending_soft_interrupts.len()
                ));
            }
            remote.clear_pending();
            remote.reset_call_output_tokens_seen();
            if !is_failover_prompt && !app.schedule_pending_remote_retry("⚠ Remote request failed.")
            {
                app.clear_pending_remote_retry();
            }
            false
        }
        ServerEvent::SessionId { session_id } => {
            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            app.note_client_focus(true);
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

            if let Some(out) = output
                && !out.is_empty()
            {
                content.push_str("\n```\n");
                content.push_str(&out);
                content.push_str("\n```");
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
            autoreview_enabled,
            autojudge_enabled,
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
            status_detail,
            upstream_provider,
            reasoning_effort,
            service_tier,
            compaction_mode,
            activity,
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
            app.note_client_focus(true);
            let session_changed = prev_session_id.as_deref() != Some(session_id.as_str());

            if session_changed {
                app.rate_limit_pending_message = None;
                app.rate_limit_reset = None;
                app.connection_type = None;
                app.status_detail = None;
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
                app.reset_streaming_tps();
                app.last_stream_activity = None;
                app.remote_resume_activity = None;
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.follow_chat_bottom();
                if prev_session_id.is_some() {
                    app.queued_messages.clear();
                    app.interleave_message = None;
                    app.clear_pending_soft_interrupt_tracking();
                }
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
            app.clear_remote_startup_phase();
            app.session.subagent_model = subagent_model;
            app.session.autoreview_enabled = autoreview_enabled;
            app.session.autojudge_enabled = autojudge_enabled;
            app.autoreview_enabled =
                autoreview_enabled.unwrap_or(crate::config::config().autoreview.enabled);
            app.autojudge_enabled =
                autojudge_enabled.unwrap_or(crate::config::config().autojudge.enabled);
            if upstream_provider.is_some() {
                app.upstream_provider = upstream_provider;
            }
            if session_changed || connection_type.is_some() {
                app.connection_type = connection_type;
            }
            if session_changed || status_detail.is_some() {
                app.status_detail = status_detail;
            }
            app.remote_reasoning_effort = reasoning_effort;
            app.remote_service_tier = service_tier;
            app.remote_compaction_mode = Some(compaction_mode);
            app.set_side_panel_snapshot(side_panel);
            app.remote_side_pane_images = images;
            app.remote_available_entries = available_models;
            app.remote_model_options = available_model_routes;
            app.remote_skills = skills;
            app.remote_sessions = all_sessions;
            app.remote_client_count = client_count;
            app.remote_is_canary = is_canary;
            app.remote_server_version = server_version;
            app.remote_server_has_update = server_has_update;
            crate::tui::workspace_client::sync_after_history(&session_id, &app.remote_sessions);

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
                if let Some(activity) = activity.filter(|activity| activity.is_processing) {
                    let current_tool_name = activity.current_tool_name.clone();
                    app.is_processing = true;
                    if app.processing_started.is_none() {
                        app.processing_started = Some(Instant::now());
                    }
                    if app.last_stream_activity.is_none() {
                        app.last_stream_activity = Some(Instant::now());
                    }
                    app.remote_resume_activity = Some(RemoteResumeActivity {
                        session_id: session_id.clone(),
                        observed_at: Instant::now(),
                        current_tool_name: current_tool_name.clone(),
                    });
                    app.status = match current_tool_name {
                        Some(tool_name) => ProcessingStatus::RunningTool(tool_name),
                        None => ProcessingStatus::Thinking(Instant::now()),
                    };
                } else {
                    app.remote_resume_activity = None;
                }
            }
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
                if messages.is_empty() && !session_changed && !app.display_messages().is_empty() {
                    crate::logging::info(
                        "Preserving locally restored display history for metadata-only History bootstrap",
                    );
                } else {
                    let restored_messages = messages
                        .into_iter()
                        .map(|msg| DisplayMessage {
                            role: msg.role,
                            content: msg.content,
                            tool_calls: msg.tool_calls.unwrap_or_default(),
                            duration_secs: None,
                            title: None,
                            tool_data: msg.tool_data,
                        })
                        .collect();
                    app.replace_display_messages(restored_messages);
                }

                if history_matches_pending_startup_prompt(app) {
                    crate::logging::info(
                        "Reload-restored startup prompt already present in server history; skipping client resubmit",
                    );
                    app.submit_input_on_startup = false;
                    app.input.clear();
                    app.cursor_pos = 0;
                    app.pending_images.clear();
                    app.set_status_notice("Reload complete — prompt preserved");
                }
            } else {
                crate::logging::info(
                    "Ignoring duplicate History event for active session after local state was restored",
                );
            }

            app.maybe_show_catchup_after_history(&session_id);

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
                app.push_display_message(DisplayMessage::error(
                    crate::tui::app::model_context::model_switch_failure_message(&err, true),
                ));
                app.set_status_notice("Model switch failed");
            } else {
                app.update_context_limit_for_model(&model);
                app.remote_provider_model = Some(model.clone());
                app.clear_remote_startup_phase();
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
            app.remote_available_entries = available_models;
            app.remote_model_options = available_model_routes;
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
            app.mark_soft_interrupt_injected(&content);
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
        ServerEvent::MemoryActivity { activity } => {
            if app.memory_enabled {
                crate::memory::apply_remote_activity_snapshot(&activity);
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

            let background_task_scope = matches!(
                &notification_type,
                crate::protocol::NotificationType::Message {
                    scope: Some(scope),
                    ..
                } if scope == "background_task"
            );

            if background_task_scope
                && crate::message::parse_background_task_notification_markdown(&message).is_some()
            {
                let presentation =
                    present_swarm_notification(&sender, &notification_type, &message);
                app.push_display_message(DisplayMessage::background_task(message.clone()));
                persist_replay_display_message(app, "background_task", None, &message);
                app.set_status_notice(presentation.status_notice);
                return false;
            }

            let presentation = present_swarm_notification(&sender, &notification_type, &message);
            app.push_display_message(DisplayMessage::swarm(
                presentation.title.clone(),
                presentation.message.clone(),
            ));
            persist_replay_display_message(
                app,
                "swarm",
                Some(presentation.title.clone()),
                &presentation.message,
            );
            app.set_status_notice(presentation.status_notice);
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
            if crate::tui::workspace_client::handle_split_response(&new_session_id) {
                finish_remote_split_launch(app);
                app.pending_split_request = false;
                app.pending_split_startup_message = None;
                app.pending_split_parent_session_id = None;
                app.pending_split_prompt = None;
                app.pending_split_model_override = None;
                app.pending_split_provider_key_override = None;
                app.pending_split_label = None;
                app.push_display_message(DisplayMessage::system(format!(
                    "Added **{}** to workspace.",
                    new_session_name,
                )));
                app.set_status_notice(format!("Workspace + {}", new_session_name));
                return false;
            }
            finish_remote_split_launch(app);
            app.pending_split_request = false;
            let startup_message = app.pending_split_startup_message.take();
            let parent_session_id_override = app.pending_split_parent_session_id.take();
            let startup_prompt = app.pending_split_prompt.take();
            let model_override = app.pending_split_model_override.take();
            let provider_key_override = app.pending_split_provider_key_override.take();
            let split_label = app.pending_split_label.take();
            if let Some(startup_message) = startup_message {
                super::commands::prepare_review_spawned_session(
                    &new_session_id,
                    startup_message,
                    model_override,
                    provider_key_override,
                    split_label.clone().map(|label| label.to_ascii_lowercase()),
                    parent_session_id_override,
                );
            } else if let Some(startup_prompt) = startup_prompt {
                App::save_startup_submission_for_session(
                    &new_session_id,
                    startup_prompt.content,
                    startup_prompt.images,
                );
            }
            let exe = super::launch_client_executable();
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
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::system(format!(
                            "🔍 {} launched in **{}**.",
                            label, new_session_name,
                        )));
                        app.set_status_notice(format!("{} launched", label));
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "✂ Split → **{}** (opened in new window)",
                            new_session_name,
                        )));
                        app.set_status_notice(format!("Split → {}", new_session_name));
                    }
                }
                Ok(false) => {
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::system(format!(
                            "🔍 {} session **{}** created.\n\nNo terminal found. Resume manually:\n```\njcode --resume {}\n```",
                            label, new_session_name, new_session_id,
                        )));
                        app.set_status_notice(format!("{} session created", label));
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "✂ Split → **{}**\n\nNo terminal found. Resume manually:\n```\njcode --resume {}\n```",
                            new_session_name, new_session_id,
                        )));
                    }
                }
                Err(e) => {
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::error(format!(
                            "{} session **{}** was created but failed to open a window: {}\n\nResume manually: `jcode --resume {}`",
                            label, new_session_name, e, new_session_id,
                        )));
                        app.set_status_notice(format!("{} open failed", label));
                    } else {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Split created **{}** but failed to open window: {}\n\nResume manually: `jcode --resume {}`",
                            new_session_name, e, new_session_id,
                        )));
                    }
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
    input::handle_text_input(app, &c.to_string());
    app.follow_chat_bottom_for_typing();
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

pub(super) async fn send_interleave_now(
    app: &mut App,
    content: String,
    remote: &mut RemoteConnection,
) {
    if content.trim().is_empty() {
        return;
    }
    let msg_clone = content.clone();
    match remote.soft_interrupt(content, false).await {
        Err(e) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to send interleave: {}",
                e
            )));
        }
        Ok(request_id) => {
            app.track_pending_soft_interrupt(request_id, msg_clone);
            app.set_status_notice("⏭ Interleave sent");
        }
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
    handle_remote_key_internal(app, code, modifiers, remote, None).await
}

pub(super) async fn handle_remote_key_event(
    app: &mut App,
    event: KeyEvent,
    remote: &mut RemoteConnection,
) -> Result<()> {
    handle_remote_key_internal(
        app,
        event.code,
        event.modifiers,
        remote,
        input::text_input_for_key_event(&event),
    )
    .await
}

async fn handle_remote_key_internal(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    remote: &mut RemoteConnection,
    text_input: Option<String>,
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

    if app.usage_overlay.is_some() {
        app.handle_usage_overlay_key(code, modifiers)?;
        return Ok(());
    }

    if let Some(ref picker) = app.inline_interactive_state
        && !picker.preview
    {
        return app.handle_inline_interactive_key(code, modifiers);
    }

    if app.handle_inline_interactive_preview_key(&code, modifiers)? {
        return Ok(());
    }

    if input::handle_visible_copy_shortcut(app, code, modifiers) {
        return Ok(());
    }

    if input::is_next_prompt_new_session_hotkey(code, modifiers) {
        app.toggle_next_prompt_new_session_routing();
        return Ok(());
    }

    if app.dictation_key_matches(code, modifiers) {
        app.handle_dictation_trigger();
        return Ok(());
    }

    if handle_workspace_navigation_key(app, code, modifiers, remote).await? {
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('m')) {
        app.toggle_side_panel();
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
            KeyCode::Char('w') | KeyCode::Char('\u{8}') | KeyCode::Backspace => {
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
        if app.activate_picker_from_preview() {
            return Ok(());
        }

        if !app.input.is_empty() {
            let prepared = input::take_prepared_input(app);

            if app.route_next_prompt_to_new_session {
                route_prepared_input_to_new_remote_session(app, remote, prepared).await?;
                return Ok(());
            }

            match app.send_action(true) {
                SendAction::Submit => submit_prepared_remote_input(app, remote, prepared).await?,
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

    // Never fall through and insert literal text for unhandled Ctrl+key chords.
    if modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(());
    }

    if let Some(text) = text_input.or_else(|| input::text_input_for_key(code, modifiers)) {
        input::handle_text_input(app, &text);
        app.follow_chat_bottom_for_typing();
        return Ok(());
    }

    if app
        .inline_interactive_state
        .as_ref()
        .map(|p| p.preview)
        .unwrap_or(false)
    {
        match code {
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                return app.handle_inline_interactive_key(code, modifiers);
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
            if app.activate_picker_from_preview() {
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
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
                    app.start_background_client_rebuild(session_id);
                    return Ok(());
                }

                if trimmed == "/update" {
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
                    app.start_background_client_update(session_id);
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
                    let _ = remote.refresh_models().await;
                    app.set_status_notice("Refreshing model catalog...");
                    app.open_model_picker();
                    return Ok(());
                }

                if super::commands::handle_usage_command(app, trimmed) {
                    return Ok(());
                }

                if super::commands::handle_agents_command(app, trimmed) {
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

                if trimmed == "/autoreview" || trimmed == "/autoreview status" {
                    app.push_display_message(DisplayMessage::system(
                        super::commands::autoreview_status_message(app),
                    ));
                    return Ok(());
                }

                if trimmed == "/autojudge" || trimmed == "/autojudge status" {
                    app.push_display_message(DisplayMessage::system(
                        super::commands::autojudge_status_message(app),
                    ));
                    return Ok(());
                }

                if trimmed == "/autoreview on" {
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Autoreview, true)
                        .await?;
                    app.set_autoreview_feature_enabled(true);
                    app.set_status_notice("Autoreview: ON");
                    app.push_display_message(DisplayMessage::system(
                        "Autoreview enabled for this session.".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed == "/autoreview off" {
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Autoreview, false)
                        .await?;
                    app.set_autoreview_feature_enabled(false);
                    app.set_status_notice("Autoreview: OFF");
                    app.push_display_message(DisplayMessage::system(
                        "Autoreview disabled for this session.".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed == "/autoreview now" {
                    let parent_session_id =
                        super::commands::current_feedback_target_session_id(app);
                    super::commands::queue_review_spawn_remote(
                        app,
                        "Autoreview",
                        parent_session_id.clone(),
                        super::commands::build_autoreview_startup_message(&parent_session_id),
                        crate::config::config().autoreview.model.clone(),
                        None,
                    );
                    if app.is_processing {
                        app.set_status_notice("Autoreview queued");
                    } else {
                        app.pending_split_request = false;
                        begin_remote_split_launch(app, "Autoreview");
                        if let Err(error) = remote.split().await {
                            finish_remote_split_launch(app);
                            app.pending_split_startup_message = None;
                            app.pending_split_parent_session_id = None;
                            app.pending_split_prompt = None;
                            app.pending_split_model_override = None;
                            app.pending_split_provider_key_override = None;
                            app.pending_split_label = None;
                            app.push_display_message(DisplayMessage::error(format!(
                                "Failed to launch autoreview session: {}",
                                error
                            )));
                            app.set_status_notice("Autoreview launch failed");
                        }
                    }
                    return Ok(());
                }

                if trimmed == "/autojudge on" {
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Autojudge, true)
                        .await?;
                    app.set_autojudge_feature_enabled(true);
                    app.set_status_notice("Autojudge: ON");
                    app.push_display_message(DisplayMessage::system(
                        "Autojudge enabled for this session.".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed == "/autojudge off" {
                    remote
                        .set_feature(crate::protocol::FeatureToggle::Autojudge, false)
                        .await?;
                    app.set_autojudge_feature_enabled(false);
                    app.set_status_notice("Autojudge: OFF");
                    app.push_display_message(DisplayMessage::system(
                        "Autojudge disabled for this session.".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed == "/autojudge now" {
                    let parent_session_id =
                        super::commands::current_feedback_target_session_id(app);
                    super::commands::queue_review_spawn_remote(
                        app,
                        "Autojudge",
                        parent_session_id.clone(),
                        super::commands::build_autojudge_startup_message(&parent_session_id),
                        crate::config::config().autojudge.model.clone(),
                        None,
                    );
                    if app.is_processing {
                        app.set_status_notice("Autojudge queued");
                    } else {
                        app.pending_split_request = false;
                        begin_remote_split_launch(app, "Autojudge");
                        if let Err(error) = remote.split().await {
                            finish_remote_split_launch(app);
                            app.pending_split_startup_message = None;
                            app.pending_split_parent_session_id = None;
                            app.pending_split_prompt = None;
                            app.pending_split_model_override = None;
                            app.pending_split_provider_key_override = None;
                            app.pending_split_label = None;
                            app.push_display_message(DisplayMessage::error(format!(
                                "Failed to launch autojudge session: {}",
                                error
                            )));
                            app.set_status_notice("Autojudge launch failed");
                        }
                    }
                    return Ok(());
                }

                if trimmed == "/review" {
                    let (model_override, provider_key_override) =
                        super::commands::preferred_one_shot_review_override()
                            .map(|(model, provider_key)| (Some(model), Some(provider_key)))
                            .unwrap_or_else(|| {
                                (crate::config::config().autoreview.model.clone(), None)
                            });
                    let parent_session_id =
                        super::commands::current_feedback_target_session_id(app);
                    super::commands::queue_review_spawn_remote(
                        app,
                        "Review",
                        parent_session_id.clone(),
                        super::commands::build_review_startup_message(&parent_session_id),
                        model_override,
                        provider_key_override,
                    );
                    if app.is_processing {
                        app.set_status_notice("Review queued");
                    } else {
                        app.pending_split_request = false;
                        begin_remote_split_launch(app, "Review");
                        if let Err(error) = remote.split().await {
                            finish_remote_split_launch(app);
                            app.pending_split_startup_message = None;
                            app.pending_split_parent_session_id = None;
                            app.pending_split_prompt = None;
                            app.pending_split_model_override = None;
                            app.pending_split_provider_key_override = None;
                            app.pending_split_label = None;
                            app.push_display_message(DisplayMessage::error(format!(
                                "Failed to launch review session: {}",
                                error
                            )));
                            app.set_status_notice("Review launch failed");
                        }
                    }
                    return Ok(());
                }

                if trimmed == "/judge" {
                    let (model_override, provider_key_override) =
                        super::commands::preferred_one_shot_review_override()
                            .map(|(model, provider_key)| (Some(model), Some(provider_key)))
                            .unwrap_or_else(|| {
                                (crate::config::config().autojudge.model.clone(), None)
                            });
                    let parent_session_id =
                        super::commands::current_feedback_target_session_id(app);
                    super::commands::queue_review_spawn_remote(
                        app,
                        "Judge",
                        parent_session_id.clone(),
                        super::commands::build_judge_startup_message(&parent_session_id),
                        model_override,
                        provider_key_override,
                    );
                    if app.is_processing {
                        app.set_status_notice("Judge queued");
                    } else {
                        app.pending_split_request = false;
                        begin_remote_split_launch(app, "Judge");
                        if let Err(error) = remote.split().await {
                            finish_remote_split_launch(app);
                            app.pending_split_startup_message = None;
                            app.pending_split_parent_session_id = None;
                            app.pending_split_prompt = None;
                            app.pending_split_model_override = None;
                            app.pending_split_provider_key_override = None;
                            app.pending_split_label = None;
                            app.push_display_message(DisplayMessage::error(format!(
                                "Failed to launch judge session: {}",
                                error
                            )));
                            app.set_status_notice("Judge launch failed");
                        }
                    }
                    return Ok(());
                }

                if trimmed.starts_with("/autoreview ") {
                    app.push_display_message(DisplayMessage::error(
                        "Usage: /autoreview [on|off|status|now]".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed.starts_with("/autojudge ") {
                    app.push_display_message(DisplayMessage::error(
                        "Usage: /autojudge [on|off|status|now]".to_string(),
                    ));
                    return Ok(());
                }

                if trimmed.starts_with("/review ") {
                    app.push_display_message(DisplayMessage::error("Usage: /review".to_string()));
                    return Ok(());
                }

                if trimmed.starts_with("/judge ") {
                    app.push_display_message(DisplayMessage::error("Usage: /judge".to_string()));
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
                    app.set_status_notice(format!("Memory: {}", label));
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

                if trimmed == "/observe"
                    || trimmed == "/observe on"
                    || trimmed == "/observe off"
                    || trimmed == "/observe status"
                {
                    let _ = super::commands::handle_session_command(app, trimmed);
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
                    if app.memory_enabled
                        && let Err(err) = remote.trigger_memory_extraction().await
                    {
                        crate::logging::info(&format!(
                            "Failed to trigger memory extraction for saved remote session: {}",
                            err
                        ));
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

                if handle_workspace_command(app, remote, trimmed).await? {
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
                        app.set_status_notice(format!("Premium: {}", label));
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
                        2. Update the todo list with `todo` if items are already done or no longer needed\n\
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

                            let mode = app
                                .improve_mode
                                .or_else(|| {
                                    app.session
                                        .improve_mode
                                        .map(super::commands::restore_improve_mode)
                                })
                                .filter(|mode| mode.is_improve());
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

                            let active_improve_mode = app
                                .improve_mode
                                .or_else(|| {
                                    app.session
                                        .improve_mode
                                        .map(super::commands::restore_improve_mode)
                                })
                                .filter(|mode| mode.is_improve());

                            if active_improve_mode.is_none()
                                && !app.is_processing
                                && !has_incomplete
                            {
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

                if let Some(command) = super::commands::parse_refactor_command(trimmed) {
                    match command {
                        Err(error) => app.push_display_message(DisplayMessage::error(error)),
                        Ok(super::commands::RefactorCommand::Resume) => {
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

                            let mode = app
                                .improve_mode
                                .or_else(|| {
                                    app.session
                                        .improve_mode
                                        .map(super::commands::restore_improve_mode)
                                })
                                .filter(|mode| mode.is_refactor());
                            let Some(mode) = mode else {
                                app.push_display_message(DisplayMessage::system(
                                    "No saved refactor run found for this session. Use `/refactor` or `/refactor plan` to start one."
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
                                super::commands::build_refactor_resume_prompt(mode, &incomplete);

                            if app.is_processing {
                                remote.cancel().await?;
                                app.set_status_notice("Interrupting for /refactor resume...");
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
                        Ok(super::commands::RefactorCommand::Status) => {
                            app.push_display_message(DisplayMessage::system(
                                super::commands::format_refactor_status(app),
                            ));
                        }
                        Ok(super::commands::RefactorCommand::Stop) => {
                            let session_id = app
                                .remote_session_id
                                .clone()
                                .unwrap_or_else(|| app.session.id.clone());
                            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
                            let has_incomplete = todos.iter().any(|todo| {
                                todo.status != "completed" && todo.status != "cancelled"
                            });

                            let active_refactor_mode = app
                                .improve_mode
                                .or_else(|| {
                                    app.session
                                        .improve_mode
                                        .map(super::commands::restore_improve_mode)
                                })
                                .filter(|mode| mode.is_refactor());

                            if active_refactor_mode.is_none()
                                && !app.is_processing
                                && !has_incomplete
                            {
                                app.push_display_message(DisplayMessage::system(
                                    "No active refactor loop to stop. Use `/refactor` to start one."
                                        .to_string(),
                                ));
                                return Ok(());
                            }

                            persist_remote_session_metadata(app, |session| {
                                session.improve_mode = None;
                            })?;
                            app.improve_mode = None;
                            let stop_prompt = super::commands::refactor_stop_prompt();
                            if app.is_processing {
                                remote.cancel().await?;
                                app.set_status_notice("Interrupting for /refactor stop...");
                                app.push_display_message(DisplayMessage::system(
                                    super::commands::refactor_stop_notice(true),
                                ));
                                app.queued_messages.push(stop_prompt);
                            } else {
                                app.push_display_message(DisplayMessage::system(
                                    super::commands::refactor_stop_notice(false),
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
                        Ok(super::commands::RefactorCommand::Run { plan_only, focus }) => {
                            let mode = super::commands::refactor_mode_for(plan_only);
                            persist_remote_session_metadata(app, |session| {
                                session.improve_mode =
                                    Some(super::commands::session_improve_mode_for(mode));
                            })?;
                            app.improve_mode = Some(mode);
                            let prompt =
                                super::commands::build_refactor_prompt(plan_only, focus.as_deref());
                            if app.is_processing {
                                remote.cancel().await?;
                                app.set_status_notice(if plan_only {
                                    "Interrupting for /refactor plan..."
                                } else {
                                    "Interrupting for /refactor..."
                                });
                                app.push_display_message(DisplayMessage::system(
                                    super::commands::refactor_launch_notice(
                                        plan_only,
                                        focus.as_deref(),
                                        true,
                                    ),
                                ));
                                app.queued_messages.push(prompt);
                            } else {
                                app.push_display_message(DisplayMessage::system(
                                    super::commands::refactor_launch_notice(
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

                if app.route_next_prompt_to_new_session {
                    route_prepared_input_to_new_remote_session(app, remote, prepared).await?;
                    return Ok(());
                }

                match app.send_action(false) {
                    SendAction::Submit => {
                        submit_prepared_remote_input(app, remote, prepared).await?
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
                .inline_interactive_state
                .as_ref()
                .map(|p| p.preview)
                .unwrap_or(false)
            {
                app.inline_interactive_state = None;
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
