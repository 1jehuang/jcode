//! Remote turn-scoped event dispatch.

use super::*;

pub(in crate::tui::app) fn handle_server_event(
    app: &mut App,
    event: ServerEvent,
    remote: &mut impl RemoteEventState,
) -> bool {
    // Local capability registration is control traffic, even when a turn is
    // active. Process its acknowledgment before the scoped-turn terminal fence.
    if let ServerEvent::Ack { id } = &event
        && remote.finish_local_tools_request(*id, true)
    {
        if crate::tui::jarvis::enabled() {
            app.set_status_notice("Jarvis voice replies ready");
            return true;
        }
        return false;
    }
    if let ServerEvent::Error { id, message, .. } = &event
        && remote.finish_local_tools_request(*id, false)
    {
        app.push_display_message(DisplayMessage::error(format!(
            "Local tool registration failed: {message}. Voice replies are not ready."
        )));
        return true;
    }
    // Route mutation errors are control responses, not failed conversation
    // turns. Handle them before the scoped-turn terminal fence.
    if let ServerEvent::Error { id, message, .. } = &event
        && app.pending_automatic_failover_request_id == Some(*id)
    {
        super::super::fail_pending_automatic_failover(app, message);
        return true;
    }
    let (event, identity) = match event {
        ServerEvent::TurnState {
            session_id,
            runtime_generation,
            turn_id,
            latest_turn_id,
        } => {
            if app.remote_turn_fence.bootstrap(
                session_id,
                runtime_generation,
                latest_turn_id,
                turn_id,
            ) {
                if !app
                    .deferred_stream_done_identity
                    .as_ref()
                    .is_some_and(|identity| app.remote_turn_fence.is_latest(identity))
                {
                    app.deferred_stream_done_id = None;
                    app.deferred_stream_done_identity = None;
                }
                if app.remote_turn_fence.active.is_none()
                    && app.current_message_id.is_none()
                    && app.deferred_stream_done_id.is_none()
                {
                    let changed = app.is_processing;
                    app.is_processing = false;
                    app.status = ProcessingStatus::Idle;
                    app.processing_started = None;
                    app.remote_resume_activity = None;
                    return changed;
                }
            }
            return false;
        }
        ServerEvent::TurnStarted { identity } => {
            let same_turn = app.remote_turn_fence.accepts(&identity);
            if !app.remote_turn_fence.start(&identity) {
                return false;
            }
            if !same_turn {
                app.deferred_stream_done_id = None;
                app.deferred_stream_done_identity = None;
            }
            app.is_processing = true;
            app.processing_started.get_or_insert_with(Instant::now);
            app.last_stream_activity = Some(Instant::now());
            app.status = ProcessingStatus::Streaming;
            return true;
        }
        ServerEvent::TurnEvent { identity, event } => {
            let paced_replay = matches!(&*event, ServerEvent::Done { .. })
                && app.deferred_stream_done_identity.as_ref() == Some(&identity)
                && app.remote_turn_fence.is_latest(&identity);
            if !app.remote_turn_fence.accepts(&identity)
                && !paced_replay
                && !(matches!(&*event, ServerEvent::Interrupted)
                    && app.remote_turn_fence.is_latest(&identity))
            {
                return false;
            }
            // Nested scopes could otherwise bypass this one validation boundary.
            if matches!(
                &*event,
                ServerEvent::TurnState { .. }
                    | ServerEvent::TurnStarted { .. }
                    | ServerEvent::TurnEvent { .. }
            ) {
                return false;
            }
            (*event, Some(identity))
        }
        event => (event, None),
    };
    if identity.is_none()
        && app.remote_turn_fence.scoped()
        && matches!(
            &event,
            ServerEvent::Interrupted
                | ServerEvent::TextDelta { .. }
                | ServerEvent::TextReplace { .. }
                | ServerEvent::TextSuffixReplace { .. }
                | ServerEvent::ReasoningDelta { .. }
                | ServerEvent::ReasoningDone { .. }
                | ServerEvent::ToolStart { .. }
                | ServerEvent::ToolInput { .. }
                | ServerEvent::ToolExec { .. }
                | ServerEvent::ToolDone { .. }
                | ServerEvent::BatchProgress { .. }
                | ServerEvent::ConnectionPhase { .. }
                | ServerEvent::StatusDetail { .. }
        )
    {
        return false;
    }
    let terminal = matches!(&event, ServerEvent::Done { .. } | ServerEvent::Error { .. });
    // Legacy control errors still use Error. Let their request-owned handlers
    // run, but never let an unscoped terminal settle an identity-aware turn.
    if terminal && identity.is_none() && app.remote_turn_fence.scoped() {
        if let ServerEvent::Error { id, message, .. } = &event {
            if app.pending_agent_model_request_id == Some(*id) {
                return handle_server_event_inner(app, event, remote);
            }
            if remote.finish_runtime_command(*id) {
                app.push_display_message(DisplayMessage::error(message.clone()));
                return true;
            }
        }
        if let ServerEvent::Done { id } | ServerEvent::Error { id, .. } = &event {
            if app.current_message_id == Some(*id) {
                if app.remote_turn_fence.active.is_none() {
                    // Validation/auth failures and synthetic control completions
                    // occur before turn acceptance, so they have no turn scope.
                    let rejected_id = *id;
                    let redraw = handle_server_event_inner(app, event, remote);
                    if !app.is_processing && app.current_message_id == Some(rejected_id) {
                        app.current_message_id = None;
                    }
                    return redraw;
                }
                if let ServerEvent::Error { message, .. } = &event {
                    let recovered = recover_rejected_queued_continuation(app);
                    if !recovered {
                        app.restore_failed_input_to_box();
                    }
                    app.clear_pending_remote_retry();
                    app.current_message_id = None;
                    remote.clear_pending();
                    // The rejected request never acquired this running turn.
                    // Preserve its stream, timing and identity-owned busy state.
                    app.is_processing = true;
                    if matches!(app.status, ProcessingStatus::Sending) {
                        app.status = ProcessingStatus::Thinking(Instant::now());
                    }
                    app.remote_resume_activity = Some(RemoteResumeActivity {
                        session_id: app.remote_session_id.as_deref().unwrap_or("").to_owned(),
                        observed_at: Instant::now(),
                        current_tool_name: None,
                    });
                    app.push_display_message(DisplayMessage::error(message.clone()));
                    return true;
                }
            }
            if app.usage_reset.invalidate_requests.remove(id).is_some() {
                app.usage_reset.refresh_usage = true;
                return true;
            }
        }
        return false;
    }
    let redraw = handle_server_event_inner(app, event, remote);
    if let Some(identity) = identity
        && terminal
    {
        if app.deferred_stream_done_id.is_some() {
            app.deferred_stream_done_identity = Some(identity);
        } else {
            app.remote_turn_fence.complete(&identity);
            app.deferred_stream_done_identity = None;
        }
    }
    redraw
}

/// Decide whether to defer applying remote session state because the server we
/// attached to is not running the binary we expect.
///
/// Precedence:
/// - The client independently measured the server's release version as strictly
///   older than its own clean release version -> defer. This wins even over the
///   server's own `server_has_update: Some(false)` self-report, because a stale
///   long-lived daemon legitimately reports "no newer binary to reload into"
///   (its `shared-server` channel still points at its own old build) while the
///   client can plainly see it is an older release. Trusting the server here is
///   exactly what left "current client, stale server" stuck (the daemon's reload
///   decision runs old code that can never drag itself forward). The newer
///   client is authoritative, so it defers and repairs the channel before
///   reloading.
/// - `Some(true)`: the server self-reported a newer binary on disk -> defer.
/// - `Some(false)`: the server is new enough to self-assess and found nothing
///   newer to reload into, AND the client could not prove it is older -> trust
///   it, do not fight it with a forced reload.
/// - `None`: the server is too old to self-report. Fall back to our own
///   client-side release-version comparison, which is the only signal that can
///   catch a pre-self-heal daemon.
pub(super) fn should_defer_history_for_runtime_identity_with_allow(
    server_has_update: Option<bool>,
    client_detected_stale: bool,
    allow_mismatch: bool,
) -> bool {
    if allow_mismatch {
        return false;
    }
    // A client-proven-older server always wins: never let an old daemon's
    // (locally correct but globally wrong) "no update" self-report veto the
    // client's own release-order comparison.
    if client_detected_stale {
        return true;
    }
    match server_has_update {
        Some(true) => true,
        Some(false) => false,
        None => false,
    }
}
