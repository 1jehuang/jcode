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
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
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
mod swarm_status_core;
mod workspace;

#[cfg(test)]
pub(super) use key_handling::reload_stale_remote_server_before_update;
use queue_recovery::{
    recover_local_interleave_to_queue, recover_stranded_soft_interrupts,
    recover_undelivered_queued_continuation,
};
// Re-export for sibling modules and tests that access reconnect state and helpers
// through `super::remote::*` without reaching into private submodules directly.
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
    Quit,
}

pub(super) async fn handle_tick(app: &mut App, remote: &mut RemoteConnection) -> bool {
    crate::tui::ui::set_frame_input_attribution(crate::tui::ui::FrameInputAttribution {
        event: Some("tick".to_string()),
        scroll_delta: None,
        model_picker_open: app
            .inline_interactive_state
            .as_ref()
            .is_some_and(|state| state.kind == crate::tui::PickerKind::Model),
    });
    let mut needs_redraw = crate::tui::periodic_redraw_required(app);
    needs_redraw |= app.flush_pending_resize_redraw();
    app.maybe_capture_runtime_memory_heartbeat();
    app.maybe_release_idle_heap();
    // Surface the cold-cache transcript warning the moment the TTL expires
    // while idle, not only when the next request starts.
    needs_redraw |= app.maybe_push_idle_cold_cache_warning();
    needs_redraw |= app.progress_copy_selection_edge_autoscroll();
    app.progress_mouse_scroll_animation();
    needs_redraw |= app.update_chat_overscroll();
    needs_redraw |= app.update_pinned_images_auto_hide();
    // Dissolve stale (off-screen) reasoning traces with zero visible motion.
    needs_redraw |= app.gc_offscreen_reasoning_traces();
    needs_redraw |= dispatch_compacted_history_load(app, remote).await;
    // Adopt the resolved scroll position once a frame containing newly loaded
    // older history has rendered, so manual scrolling resumes seamlessly.
    needs_redraw |= app.reconcile_history_anchor();
    // Reveal buffered streaming text at the smooth paced rate on each tick, the
    // same as the local turn loop. Finalization paths still call flush().
    let ops = app.stream_buffer.flush_smooth_frame();
    if app.apply_stream_ops(ops) {
        needs_redraw = true;
    }

    needs_redraw |= app.refresh_todos_view_if_needed();
    needs_redraw |= app.refresh_todo_card_if_needed();
    needs_redraw |= app.refresh_side_panel_linked_content_if_due();
    needs_redraw |= app.poll_model_picker_load();
    needs_redraw |= app.poll_session_picker_load();
    needs_redraw |= app.poll_session_picker_presence();
    needs_redraw |= app.onboarding_tick();

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

        if let Some(target_session) = app.workspace_client.take_pending_resume_session() {
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
            if matches!(app.status, ProcessingStatus::WaitingForNetwork { .. })
                && !crate::network_retry::is_probably_online().await
            {
                app.schedule_pending_remote_network_wait("network probe still failing");
                return true;
            }
            if matches!(app.status, ProcessingStatus::WaitingForNetwork { .. }) {
                app.status = ProcessingStatus::Idle;
                app.status_detail = None;
            }
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
        if begin_remote_send(
            app,
            remote,
            combined.clone(),
            vec![],
            true,
            reminder.clone(),
            auto_retry,
            0,
        )
        .await
        .is_err()
        {
            // The send never reached the server (e.g. the socket died under a
            // reload handoff). Dropping the dequeued messages here would lose
            // them permanently (issue #391); put them back so the queue
            // re-dispatches after reconnect.
            crate::logging::error(
                "Failed to send queued continuation message; restoring it to the queue",
            );
            if let Some(reminder) = reminder {
                app.hidden_queued_system_messages.insert(0, reminder);
            }
            if !combined.is_empty() {
                app.queued_messages.insert(0, combined);
            }
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
            Some(combined.clone()),
            true,
            0,
        )
        .await
        .is_err()
        {
            crate::logging::error(
                "Failed to send hidden continuation reminder; restoring it to the queue",
            );
            app.hidden_queued_system_messages.insert(0, combined);
        }
        needs_redraw = true;
    }

    detect_and_cancel_stall(app, remote).await;
    needs_redraw |= recover_stuck_remote_history(app, remote).await;
    needs_redraw
}

/// Forward the reasoning-effort variant staged by a model-picker selection
/// (e.g. "gpt-5.5 (high)") to the server right after the model-switch request.
/// In remote mode the picker cannot apply effort to `app.provider` (a local
/// stand-in), so skipping this leaves the server on its configured default
/// effort - typically low - silently downgrading the request (issue #427).
async fn forward_pending_reasoning_effort(app: &mut App, remote: &mut RemoteConnection) {
    let Some(effort) = app.pending_reasoning_effort.take() else {
        return;
    };
    match remote.set_reasoning_effort(&effort).await {
        Ok(()) => {
            // Optimistically track the requested effort so the widget/header
            // reflect the picker choice; ReasoningEffortChanged confirms it.
            app.remote_reasoning_effort = Some(effort);
        }
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to request reasoning effort '{}': {}",
                effort, error
            )));
            app.set_status_notice("Effort switch failed");
        }
    }
}

pub(super) async fn handle_terminal_event(
    app: &mut App,
    _terminal: &mut DefaultTerminal,
    remote: &mut RemoteConnection,
    event: Option<std::result::Result<Event, std::io::Error>>,
) -> Result<bool> {
    let mut needs_redraw = false;
    let mut input_attribution = crate::tui::ui::FrameInputAttribution {
        event: None,
        scroll_delta: None,
        model_picker_open: app
            .inline_interactive_state
            .as_ref()
            .is_some_and(|state| state.kind == crate::tui::PickerKind::Model),
    };
    match event {
        Some(Ok(Event::FocusGained)) => {
            input_attribution.event = Some("focus_gained".to_string());
            needs_redraw |= app.set_client_focused(true);
            app.note_client_focus(true);
        }
        Some(Ok(Event::FocusLost)) => {
            input_attribution.event = Some("focus_lost".to_string());
            app.set_client_focused(false);
        }
        Some(Ok(Event::Key(key))) => {
            input_attribution.event = Some(format!("key:{:?}:{:?}", key.code, key.kind));
            input_attribution.scroll_delta = key_scroll_delta(&key);
            app.note_client_interaction();
            app.update_copy_badge_key_event(key);
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                handle_remote_key_event(app, key, remote).await?;
                if let Some(selection) = app.pending_route_selection.take() {
                    app.pending_model_switch = None;
                    match remote.set_route_selection(selection).await {
                        Ok(_) => {
                            app.remote_model_switch_in_flight = true;
                            forward_pending_reasoning_effort(app, remote).await;
                        }
                        Err(error) => {
                            app.pending_reasoning_effort = None;
                            // A fallback-offer resend must not fire without its
                            // route switch; drop it with the failed request.
                            app.pending_fallback_resend = None;
                            app.push_display_message(DisplayMessage::error(format!(
                                "Failed to request model switch: {}",
                                error
                            )));
                            app.set_status_notice("Model switch failed");
                        }
                    }
                } else if let Some(spec) = app.pending_model_switch.take() {
                    match remote.set_model(&spec).await {
                        Ok(_) => {
                            app.remote_model_switch_in_flight = true;
                            forward_pending_reasoning_effort(app, remote).await;
                        }
                        Err(error) => {
                            app.pending_reasoning_effort = None;
                            app.push_display_message(DisplayMessage::error(format!(
                                "Failed to request model switch: {}",
                                error
                            )));
                            app.set_status_notice("Model switch failed");
                        }
                    }
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
            input_attribution.event = Some(format!("paste:{}", text.len()));
            app.note_client_interaction();
            app.handle_paste(text);
            needs_redraw = true;
        }
        Some(Ok(Event::Mouse(mouse))) => {
            input_attribution.event = Some(format!("mouse:{:?}", mouse.kind));
            input_attribution.scroll_delta = mouse_scroll_delta(&mouse);
            if !matches!(mouse.kind, MouseEventKind::Moved) {
                app.note_client_interaction();
                handle_mouse_event(app, mouse);
                needs_redraw = true;
                needs_redraw |= dispatch_compacted_history_load(app, remote).await;
            }
        }
        Some(Ok(Event::Resize(_, _))) => {
            input_attribution.event = Some("resize".to_string());
            needs_redraw = app.should_redraw_after_resize();
        }
        Some(Err(error)) => {
            input_attribution.event = Some(format!("event_error:{}", error));
        }
        _ => {
            input_attribution.event = Some("none".to_string());
        }
    }
    crate::tui::ui::set_frame_input_attribution(input_attribution);
    Ok(needs_redraw)
}

fn key_scroll_delta(key: &KeyEvent) -> Option<i32> {
    match key.code {
        KeyCode::Up => Some(-1),
        KeyCode::PageUp => Some(-10),
        KeyCode::Down => Some(1),
        KeyCode::PageDown => Some(10),
        _ => None,
    }
}

fn mouse_scroll_delta(mouse: &MouseEvent) -> Option<i32> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(-1),
        MouseEventKind::ScrollDown => Some(1),
        MouseEventKind::ScrollLeft => Some(-1),
        MouseEventKind::ScrollRight => Some(1),
        _ => None,
    }
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
) -> bool {
    match bus_event {
        Ok(BusEvent::UsageReport(results)) => {
            app.handle_usage_report(results);
            true
        }
        Ok(BusEvent::ClipboardPasteCompleted(result)) => {
            app.handle_clipboard_paste_completed(result)
        }
        Ok(BusEvent::ModelRefreshCompleted(result)) => {
            app.handle_model_refresh_completed(result);
            true
        }
        Ok(BusEvent::UiActivity(activity)) => super::local::handle_ui_activity(app, activity),
        Ok(BusEvent::GitStatusCompleted(result)) => {
            super::commands::handle_git_status_completed(app, result);
            true
        }
        Ok(BusEvent::MermaidRenderCompleted) => true,
        Ok(BusEvent::UsageReportProgress(progress)) => {
            app.handle_usage_report_progress(progress);
            true
        }
        Ok(BusEvent::LoginCompleted(login)) => {
            let success = login.success && login.provider != "copilot_code";
            let provider_hint = auth_provider_hint_for_login_provider(&login.provider);
            let auth = auth_changed_event_for_login_provider(&login.provider);
            let prefer_strongest = success && app.onboarding_should_prefer_strongest_model();
            app.handle_login_completed(login);
            if success
                && let Err(error) = remote
                    .notify_auth_changed_event(provider_hint, auth, prefer_strongest)
                    .await
            {
                crate::logging::warn(&format!(
                    "Failed to notify server about refreshed auth: {error}"
                ));
                app.finish_auth_catalog_refresh();
                app.set_status_notice("Model setup will retry after reconnect");
            }
            true
        }
        Ok(BusEvent::OnboardingModelValidated(result)) => {
            app.handle_onboarding_model_validated(result)
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
            match remote.send_transcript(text, mode).await {
                Ok(()) => app.mark_dictation_delivered(),
                Err(error) => app.handle_dictation_failure(error.to_string()),
            }
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
        _ => false,
    }
}

/// Resolve the canonical auth provider id the server uses to attribute an
/// auth-change refresh for a completed login.
///
/// `LoginCompleted.provider` is the login descriptor's display label (e.g.
/// "Anthropic API"), id, or alias - not the canonical server provider id. This
/// used to only map Azure and OpenAI-compatible logins, so direct logins
/// (Claude OAuth/API key, OpenAI, OpenRouter, Bedrock, ...) sent no hint. With
/// no hint the server fell back to the session's currently active provider,
/// mislabeling the catalog-refresh message ("OpenAI credentials are active"
/// after an Anthropic API-key login) and skipping the post-login model switch.
fn auth_provider_hint_for_login_provider(provider: &str) -> Option<&'static str> {
    let provider = provider.trim();
    // Azure's runtime id ("azure-openai") differs from its login descriptor id
    // ("azure"); keep the dedicated mapping used across the auth lifecycle.
    if provider.eq_ignore_ascii_case("azure")
        || provider.eq_ignore_ascii_case("azure-openai")
        || provider.eq_ignore_ascii_case("azure openai")
    {
        return Some("azure-openai");
    }

    use crate::provider_catalog::LoginProviderTarget;
    let descriptor = crate::provider_catalog::resolve_login_provider_loose(provider)?;
    match descriptor.target {
        LoginProviderTarget::Azure => Some("azure-openai"),
        // OpenAI-compatible profiles carry their own catalog namespace id.
        LoginProviderTarget::OpenAiCompatible(profile) => Some(profile.id),
        // Auto-import has no single runtime to attribute the refresh to.
        LoginProviderTarget::AutoImport => None,
        _ => Some(descriptor.id),
    }
}

fn auth_changed_event_for_login_provider(provider: &str) -> Option<crate::protocol::AuthChanged> {
    use crate::provider_catalog::LoginProviderTarget;
    let provider_id = auth_provider_hint_for_login_provider(provider)?;
    let mut auth = crate::protocol::AuthChanged::new(provider_id);
    // These fields are informational; the server routes off `provider` and the
    // `expected_*` hints. Reflect the descriptor's auth kind so OAuth logins are
    // not recorded as API-key pastes.
    let descriptor = crate::provider_catalog::resolve_login_provider_loose(provider);
    let api_key_login = descriptor
        .map(|descriptor| {
            use crate::provider_catalog::LoginProviderAuthKind;
            matches!(
                descriptor.auth_kind,
                LoginProviderAuthKind::ApiKey | LoginProviderAuthKind::Hybrid
            )
        })
        .unwrap_or(true);
    if api_key_login {
        auth.auth_method = Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey);
        auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
    }
    // Only logins whose descriptor actually targets the OpenAI-compatible
    // runtime claim its namespace. Do not key this off
    // `openai_compatible_profile_by_id`: native providers (`anthropic-api`,
    // `openai-api`) alias doctor-probe compat profiles with the same id, but
    // their auth activation deliberately routes through the native runtime.
    if provider_id == "azure-openai" {
        auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new("azure-openai"));
        auth.expected_catalog_namespace =
            Some(crate::protocol::CatalogNamespace::new("azure-openai"));
    } else if descriptor
        .is_some_and(|d| matches!(d.target, LoginProviderTarget::OpenAiCompatible(_)))
    {
        auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new(
            "openai-compatible",
        ));
        auth.expected_catalog_namespace = Some(crate::protocol::CatalogNamespace::new(provider_id));
    }
    Some(auth)
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
            needs_redraw |= app.set_client_focused(true);
            app.note_client_focus(true);
        }
        Some(Ok(Event::FocusLost)) => {
            app.set_client_focused(false);
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
            if !matches!(mouse.kind, MouseEventKind::Moved) {
                app.note_client_interaction();
                handle_mouse_event(app, mouse);
                needs_redraw = true;
            }
        }
        Some(Ok(Event::Resize(_, _))) => {
            needs_redraw = app.should_redraw_after_resize();
        }
        None => {
            // Input EOF: if the controlling terminal is gone this client is an
            // orphan (window died without a deliverable SIGHUP). Quit instead
            // of reconnect-looping forever with no way to ever receive input.
            if super::terminal_liveness::terminal_abandoned() {
                crate::logging::warn(
                    "Terminal input closed and controlling terminal is gone while disconnected; exiting orphaned client",
                );
                app.should_quit = true;
            }
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
    terminal: &mut Terminal<B>,
    remote: &mut RemoteConnection,
    state: &mut RemoteRunState,
    event: RemoteRead,
) -> Result<(RemoteEventOutcome, bool)> {
    match event {
        RemoteRead::Disconnected(reason) => {
            if let RemoteDisconnectReason::Protocol(error) = &reason {
                let detail = format_disconnect_reason(&reason);
                crate::logging::error(&format!(
                    "Remote protocol error is not retryable; stopping reconnect loop: {}",
                    error
                ));
                app.push_display_message(DisplayMessage::error(format!(
                    "Remote protocol error. Stopped reconnecting to avoid replaying a large/corrupt session repeatedly. {}\n\nTry starting a fresh session, or resume after reducing/removing oversized tool output from the session history.",
                    detail
                )));
                app.set_status_notice("Remote protocol error");
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                return Ok((RemoteEventOutcome::Quit, true));
            }
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
            // Frame-oriented debug commands used to enable visual debugging and
            // immediately read the frame buffer. On the first request the buffer
            // is still empty because no draw has happened since enabling capture.
            // Render once before producing the response so callers always receive
            // the current TUI state rather than "no frames captured".
            if debug_command_needs_current_frame(&command) {
                crate::tui::visual_debug::enable();
                if let Err(error) = terminal.draw(|frame| crate::tui::ui::draw(frame, app)) {
                    let output = format!("ERR: failed to capture current frame: {error}");
                    let _ = remote.send_client_debug_response(id, output).await;
                    return Ok((RemoteEventOutcome::Continue, false));
                }
            }
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

pub(super) fn debug_command_needs_current_frame(command: &str) -> bool {
    matches!(
        command.trim(),
        "frame"
            | "frame-normalized"
            | "screen"
            | "screen-json"
            | "screen-json-normalized"
            | "layout"
            | "margins"
            | "widgets"
            | "info-widgets"
            | "render-stats"
            | "render-order"
            | "anomalies"
            | "theme"
    )
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
    // A disconnect can drop the final auth catalog completion event. Keep any
    // queued prompt for reconnect, but release the transient model-setup gate so
    // it cannot remain blocked forever waiting for an event that was lost.
    app.finish_auth_catalog_refresh();
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

/// First wait before the watchdog re-requests history. Generous enough that a
/// normal (slow) bootstrap completes on its own, short enough that a genuinely
/// stuck session recovers in seconds instead of requiring a manual `/restart`.
const REMOTE_HISTORY_RECOVERY_FIRST_DELAY: Duration = Duration::from_secs(6);
/// Spacing between subsequent re-requests after the first one.
const REMOTE_HISTORY_RECOVERY_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// How many times we re-request history before giving up and telling the user
/// to `/restart`. Bounded so a server that genuinely never answers does not
/// spin forever.
const REMOTE_HISTORY_RECOVERY_MAX_ATTEMPTS: u32 = 4;

/// Watchdog for the "stuck on loading session…" bug.
///
/// Every remote prompt path is gated behind `RemoteConnection::has_loaded_history()`:
/// `submit_prepared_remote_input` parks prompts in `pending_prompt_before_history`,
/// and `process_remote_followups` returns early at the `!has_loaded_history()`
/// gate. That gate only clears when the server delivers a `History` event. If
/// that event never arrives after a (re)connect or reload handoff (dropped
/// event, momentarily busy agent, or a path that returned without history), the
/// client is stuck forever showing "loading session…" and the only escape is a
/// manual `/restart`.
///
/// This watchdog detects a connection that has been waiting too long for the
/// bootstrap history and re-requests it a bounded number of times. If history
/// still never loads, it surfaces an actionable message instead of leaving the
/// user staring at a frozen header.
async fn recover_stuck_remote_history(app: &mut App, remote: &mut RemoteConnection) -> bool {
    // Once history has loaded the watchdog has nothing to do; make sure its
    // budget is cleared so a later rewind-triggered reload starts fresh.
    if remote.has_loaded_history() {
        if app.remote_history_wait_started.is_some() {
            app.clear_remote_history_wait();
        }
        return false;
    }

    // A pending server reload intentionally leaves history unloaded until the
    // reload fires (see `process_remote_followups`); don't fight that path.
    if app.pending_server_reload {
        return false;
    }

    // Only meaningful for an established remote client connection. During the
    // initial connect/reconnect handshake the run loop drives history loading
    // directly, and there is no point re-requesting before we've attached.
    if !app.is_remote {
        return false;
    }

    let now = Instant::now();
    let waited = match app.remote_history_wait_started {
        Some(started) => now.saturating_duration_since(started),
        None => {
            // Begin tracking from the first tick that observes unloaded history
            // on a live connection.
            app.remote_history_wait_started = Some(now);
            return false;
        }
    };

    if waited < REMOTE_HISTORY_RECOVERY_FIRST_DELAY {
        return false;
    }

    // A large newline-delimited History event may take longer than the
    // watchdog delay to arrive. Once any part of a frame is buffered, the
    // response was not dropped: it is actively being assembled by the reader.
    // Re-requesting here queues another complete (potentially tens-of-MB)
    // History payload behind the first and can keep the connection saturated
    // for minutes. Let the in-flight frame finish instead.
    if remote.has_buffered_inbound_frame() {
        return false;
    }

    if app.remote_history_recovery_attempts >= REMOTE_HISTORY_RECOVERY_MAX_ATTEMPTS {
        // We've exhausted re-requests. Surface a one-time actionable hint so the
        // user isn't stuck on a silent "loading session…" forever.
        if app.remote_history_recovery_last_attempt.is_some() {
            crate::logging::warn(
                "Remote history never loaded after repeated re-requests; session is stuck on \
                 'loading session…'. Advising /restart.",
            );
            app.push_display_message(DisplayMessage::system(
                "⚠ Still loading session… the server hasn't sent the conversation history. \
                 This usually clears on its own; if it persists, run /restart to reconnect."
                    .to_string(),
            ));
            app.set_status_notice("Session history not loading - try /restart");
            // Clear last_attempt so we don't repeat the message every tick, but
            // keep attempts at max so we don't re-enter the retry path.
            app.remote_history_recovery_last_attempt = None;
            return true;
        }
        return false;
    }

    // Rate-limit re-requests so we don't flood the server.
    if let Some(last) = app.remote_history_recovery_last_attempt
        && now.saturating_duration_since(last) < REMOTE_HISTORY_RECOVERY_RETRY_INTERVAL
    {
        return false;
    }

    app.remote_history_recovery_attempts += 1;
    app.remote_history_recovery_last_attempt = Some(now);
    crate::logging::warn(&format!(
        "Remote history still not loaded after {}s; re-requesting session history (attempt {}/{}, session={:?})",
        waited.as_secs(),
        app.remote_history_recovery_attempts,
        REMOTE_HISTORY_RECOVERY_MAX_ATTEMPTS,
        app.remote_session_id,
    ));
    match remote.request_history().await {
        Ok(_) => {
            app.set_status_notice("Loading session… re-requesting history");
        }
        Err(err) => {
            crate::logging::error(&format!(
                "History recovery re-request failed: {err}; will retry on next watchdog tick"
            ));
        }
    }
    true
}

/// Record (once per distinct reason) why the restored startup auto-submit is
/// being deferred. This makes "headed-spawn prompt seen but never sent" cases
/// debuggable without spamming a log line on every event-loop tick.
fn note_startup_submit_deferred(app: &mut App, reason: &'static str) {
    if !app.submit_input_on_startup {
        // Nothing pending; clear any stale reason so the next deferral logs.
        app.startup_submit_deferred_reason = None;
        return;
    }
    if app.startup_submit_deferred_reason == Some(reason) {
        return;
    }
    app.startup_submit_deferred_reason = Some(reason);
    crate::logging::info(&format!(
        "Startup auto-submit deferred: {reason} (input_chars={}, pending_images={})",
        app.input.chars().count(),
        app.pending_images.len(),
    ));
}

/// Fire a pending server reload (newer binary on disk, or a server/client
/// runtime-identity mismatch that the History handler intentionally deferred).
///
/// Extracted so it can run BEFORE the `has_loaded_history` gate in
/// `process_remote_followups`: the runtime-identity defer path never marks
/// history as loaded, so the reload must be allowed to fire while history is
/// still pending, otherwise the connection stalls indefinitely on
/// "Loading session...".
async fn dispatch_pending_server_reload(app: &mut App, remote: &mut RemoteConnection) {
    app.pending_server_reload = false;
    if app.auto_server_reload {
        // Defense-in-depth for issue #277: a correct reload only needs to
        // happen once. If we keep being told a newer binary is available
        // after several auto-reloads, treat it as a false-positive loop and
        // stop hammering the server (which would otherwise flicker the UI).
        const MAX_AUTO_SERVER_RELOADS: u32 = 3;
        if app.server_auto_reload_attempts >= MAX_AUTO_SERVER_RELOADS {
            crate::logging::warn(&format!(
                "Suppressing server auto-reload after {} attempts; server keeps reporting an update (possible reload loop, issue #277)",
                app.server_auto_reload_attempts
            ));
            app.push_display_message(DisplayMessage::system(
                "ℹ Server keeps reporting a newer binary after repeated reloads; auto-reload paused to avoid a loop. Use `/reload` manually if needed.".to_string(),
            ));
            app.set_status_notice("Server auto-reload paused (possible loop)");
        } else {
            app.server_auto_reload_attempts += 1;
            app.append_reload_message("Reloading server with newer binary...");
            if let Err(err) = remote.reload().await {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to auto-reload server: {}. Use `/reload` to retry.",
                    err
                )));
                app.set_status_notice("Server update available - auto reload failed");
            }
        }
    } else {
        app.push_display_message(DisplayMessage::system(
            "ℹ Newer server binary detected. Auto-reload is disabled by `display.auto_server_reload = false`. Use `/reload` manually when you're ready.".to_string(),
        ));
        app.set_status_notice("Server update available - manual /reload recommended");
    }
}

pub(super) async fn process_remote_followups(app: &mut App, remote: &mut RemoteConnection) {
    // A pending *server* reload must be dispatched even when the bootstrap
    // History payload was intentionally deferred. The runtime-identity /
    // stale-binary guard in the History handler sets `pending_server_reload =
    // true` and returns WITHOUT marking history as loaded, by design: we want to
    // reload the server before applying any session state. If the
    // `has_loaded_history` gate below ran first, the reload would never fire,
    // history would stay unloaded forever, and every typed prompt would be stuck
    // behind "Loading session..." until the user restarted (server/client binary
    // mismatch reload-handoff stall).
    if app.pending_server_reload && !app.is_processing {
        dispatch_pending_server_reload(app, remote).await;
        return;
    }

    if !remote.has_loaded_history() {
        note_startup_submit_deferred(app, "remote history not loaded yet");
        return;
    }

    let _ = recover_stranded_soft_interrupts(app, remote).await;

    if app.pending_queued_dispatch {
        note_startup_submit_deferred(app, "pending_queued_dispatch in progress");
        return;
    }

    if !app.remote_model_switch_in_flight
        && !app.is_processing
        && let Some(payload) = app.pending_fallback_resend.take()
    {
        // A fallback offer was accepted and the server confirmed the route
        // switch: resend the failed turn's payload on the new route. The
        // original user message is already in the transcript from the failed
        // attempt, so do not echo it again; just clear the input box if the
        // error path restored the prompt there.
        if let Some(raw_input) = payload.raw_input.as_deref()
            && app.input == raw_input
        {
            app.input.clear();
            app.cursor_pos = 0;
        }
        app.last_submitted_input = payload.raw_input.clone();
        crate::logging::info("Resending failed turn after accepted fallback route switch");
        if let Err(error) = begin_remote_send(
            app,
            remote,
            payload.content,
            payload.images,
            payload.is_system,
            payload.system_reminder,
            payload.auto_retry,
            0,
        )
        .await
        {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to resend after fallback switch: {}",
                error
            )));
            app.set_status_notice("Fallback resend failed");
        }
        return;
    }

    if !app.remote_model_switch_in_flight
        && !app.auth_catalog_refresh_pending
        && !app.is_processing
        && let Some(prepared) = app.pending_prompt_after_model_switch.take()
    {
        if let Err(error) = submit_prepared_remote_input(app, remote, prepared).await {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to submit prompt after model switch: {}",
                error
            )));
            app.set_status_notice("Queued prompt failed");
        }
        return;
    }

    // Now that history is loaded (guaranteed by the gate above), dispatch any
    // prompt that the user submitted during the pre-history window. Submitting
    // it earlier would have been clobbered by the bootstrap History payload's
    // `session_changed` clear; deferring here fixes the intermittent
    // "first prompt vanishes / weird render" bug.
    if !app.is_processing
        && let Some(prepared) = app.pending_prompt_before_history.take()
    {
        crate::logging::info(
            "Dispatching prompt that was held until remote history finished loading",
        );
        if let Err(error) = submit_prepared_remote_input(app, remote, prepared).await {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to submit prompt after session load: {}",
                error
            )));
            app.set_status_notice("Prompt failed");
        }
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
        app.startup_submit_deferred_reason = None;
        if !app.input.is_empty() || !app.pending_images.is_empty() {
            crate::logging::info(&format!(
                "Startup auto-submit firing: input_chars={} pending_images={}",
                app.input.chars().count(),
                app.pending_images.len(),
            ));
            let prepared = input::take_prepared_input(app);
            if let Err(error) = submit_prepared_remote_input(app, remote, prepared).await {
                crate::logging::warn(&format!("Startup auto-submit failed: {error}"));
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to submit startup prompt: {}",
                    error
                )));
                app.set_status_notice("Startup prompt failed");
            }
            return;
        } else {
            crate::logging::warn(
                "Startup auto-submit skipped: submit flag was set but input and pending images are both empty",
            );
        }
    } else if app.submit_input_on_startup && app.is_processing {
        note_startup_submit_deferred(app, "session still processing (is_processing=true)");
    }

    if app.pending_background_client_reload.is_some() && !app.is_processing {
        app.maybe_finish_background_client_reload();
        return;
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
        if begin_remote_send(
            app,
            remote,
            combined.clone(),
            vec![],
            true,
            reminder.clone(),
            auto_retry,
            0,
        )
        .await
        .is_err()
        {
            // Do not drop a dequeued follow-up whose send never reached the
            // server (issue #391); restore it for redispatch after reconnect.
            crate::logging::error(
                "Failed to send queued continuation message; restoring it to the queue",
            );
            if let Some(reminder) = reminder {
                app.hidden_queued_system_messages.insert(0, reminder);
            }
            if !combined.is_empty() {
                app.queued_messages.insert(0, combined);
            }
        }
    } else if !app.hidden_queued_system_messages.is_empty() {
        let reminders = std::mem::take(&mut app.hidden_queued_system_messages);
        let combined = reminders.join("\n\n");
        if begin_remote_send(
            app,
            remote,
            String::new(),
            vec![],
            true,
            Some(combined.clone()),
            true,
            0,
        )
        .await
        .is_err()
        {
            crate::logging::error(
                "Failed to send hidden continuation reminder; restoring it to the queue",
            );
            app.hidden_queued_system_messages.insert(0, combined);
        }
    }
}

/// Client-side stall budget before the TUI cancels an in-flight turn.
///
/// The server relays provider events over the local socket; when the upstream
/// model reasons silently, no events cross the socket, so a hardcoded short
/// watchdog cannot distinguish a dead connection from a healthy long think
/// (issue #434). Derive the budget from the provider-agnostic
/// `[provider] stream_idle_timeout_secs` / `JCODE_STREAM_IDLE_TIMEOUT_SECS`
/// setting plus grace time so the server-side idle timeout (which produces a
/// visible error event) always fires first. Never below 2 minutes.
fn stall_timeout() -> Duration {
    const MIN_STALL_TIMEOUT: Duration = Duration::from_secs(2 * 60);
    const GRACE: Duration = Duration::from_secs(30);
    let provider_idle = crate::provider::stream_idle_timeout();
    provider_idle.saturating_add(GRACE).max(MIN_STALL_TIMEOUT)
}

/// Human-readable stall duration for user-facing stall messages, e.g.
/// "2 minutes", "3.5 minutes", or "90 seconds".
fn format_stall_duration(timeout: Duration) -> String {
    let secs = timeout.as_secs();
    if secs < 120 {
        format!("{} seconds", secs)
    } else if secs.is_multiple_of(60) {
        format!("{} minutes", secs / 60)
    } else {
        format!("{:.1} minutes", secs as f64 / 60.0)
    }
}

async fn detect_and_cancel_stall(app: &mut App, remote: &mut RemoteConnection) {
    let stall_timeout = stall_timeout();
    let is_running_tool = matches!(app.status, ProcessingStatus::RunningTool(_));
    if app.is_processing && !is_running_tool {
        let stalled = app
            .last_stream_activity
            .map(|t| t.elapsed() > stall_timeout)
            .unwrap_or_else(|| {
                app.processing_started
                    .map(|t| t.elapsed() > stall_timeout)
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
            let _ = remote.cancel_with_reason("stall_guard").await;
            app.is_processing = false;
            app.clear_visible_turn_started();
            app.status = ProcessingStatus::Idle;
            app.current_message_id = None;
            app.processing_started = None;
            app.last_stream_activity = None;
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
            let stall_desc = format_stall_duration(stall_timeout);
            if !app.schedule_pending_remote_retry(&format!(
                "⚠ Stream stalled (no response for {stall_desc}). Processing cancelled.",
            )) {
                // Keep a dispatched-but-unfinished queued follow-up on the
                // queue instead of silently dropping it (issue #391).
                let recovered = recover_undelivered_queued_continuation(app, "stream stall");
                app.clear_pending_remote_retry();
                if recovered {
                    app.push_display_message(DisplayMessage::system(format!(
                        "⚠ Stream stalled (no response for {stall_desc}). Processing cancelled. Your queued follow-up stays queued.",
                    )));
                } else {
                    app.push_display_message(DisplayMessage::system(format!(
                        "⚠ Stream stalled (no response for {stall_desc}). Processing cancelled. You can resend your message. Raise `[provider] stream_idle_timeout_secs` in config.toml if your model thinks silently for longer.",
                    )));
                }
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
            "version": jcode_build_meta::version(),
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
        || super::commands::handle_keys_command(app, trimmed)
        || super::commands::handle_session_command(app, trimmed)
        || super::commands::handle_test_command(app, trimmed)
        || super::commands::handle_disabled_mission_command(app, trimmed)
        || super::commands::handle_goals_command(app, trimmed)
        || super::commands::handle_config_command(app, trimmed)
        || super::commands::handle_log_command(app, trimmed)
        || super::commands::handle_diff_command(app, trimmed)
        || super::commands::handle_debug_command(app, trimmed)
        || super::commands::handle_model_command(app, trimmed)
        || super::commands::handle_usage_command(app, trimmed)
        || super::commands::handle_feedback_command(app, trimmed)
        || super::support::handle_support_command(app, trimmed)
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
    input::promote_dropped_images(app);
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

    let macos_option_shortcut =
        crate::tui::keybind::shortcut_char_for_macos_option_key(code, modifiers);
    if modifiers.contains(KeyModifiers::ALT) && input::handle_alt_key(app, code) {
        return Ok(());
    }
    if let Some(shortcut) = macos_option_shortcut
        && input::handle_alt_key(app, KeyCode::Char(shortcut))
    {
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::SUPER) {
        match code {
            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('\u{7f}') => {
                input::delete_input_to_start(app);
                return Ok(());
            }
            KeyCode::Left | KeyCode::Home | KeyCode::Char('a') => {
                app.cursor_pos = 0;
                return Ok(());
            }
            KeyCode::Right | KeyCode::End | KeyCode::Char('e') => {
                app.cursor_pos = app.input.len();
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
            KeyCode::Char('v') => {
                app.paste_from_clipboard();
                return Ok(());
            }
            _ => {}
        }
    }

    if code == KeyCode::Enter && modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) {
        queue_message_for_reconnect(app);
        return Ok(());
    }

    if app.new_terminal_key_matches(code, modifiers) {
        app.handle_new_terminal_hotkey();
        return Ok(());
    }

    if app.open_resume_key_matches(code, modifiers) {
        app.open_session_picker();
        return Ok(());
    }

    if code == KeyCode::Enter && modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) {
        input::insert_input_text(app, "\n");
        return Ok(());
    }

    if let Some(text) = text_input.or_else(|| input::text_input_for_key(code, modifiers)) {
        input::handle_text_input(app, &text);
        app.follow_chat_bottom_for_typing();
        return Ok(());
    }

    // Never fall through and insert literal text for unhandled Ctrl+key chords. This stays after
    // text_input so Ctrl+Alt/AltGr symbols delivered as final printable text still work.
    if modifiers.contains(KeyModifiers::CONTROL) {
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

#[cfg(test)]
mod stall_guard_tests {
    use super::*;

    #[test]
    fn stall_timeout_never_below_two_minutes() {
        // Even with the default 180s provider idle timeout, the client stall
        // guard must give the server-side timeout room to fire first.
        let timeout = stall_timeout();
        assert!(
            timeout >= Duration::from_secs(2 * 60),
            "stall timeout regressed below 2 minutes: {timeout:?}"
        );
        // And it must exceed the provider idle timeout so a healthy silent
        // reasoning stretch is cancelled server-side (visible error + retry)
        // rather than by the client watchdog (issue #434).
        let provider_idle = crate::provider::stream_idle_timeout();
        assert!(
            timeout > provider_idle,
            "stall timeout {timeout:?} must exceed provider idle timeout {provider_idle:?}"
        );
    }

    #[test]
    fn format_stall_duration_is_human_readable() {
        assert_eq!(format_stall_duration(Duration::from_secs(90)), "90 seconds");
        assert_eq!(format_stall_duration(Duration::from_secs(120)), "2 minutes");
        assert_eq!(
            format_stall_duration(Duration::from_secs(210)),
            "3.5 minutes"
        );
        assert_eq!(
            format_stall_duration(Duration::from_secs(430)),
            "7.2 minutes"
        );
    }
}
