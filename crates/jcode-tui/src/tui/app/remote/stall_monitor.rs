use super::*;

/// Client-side stall budget before the TUI cancels an in-flight turn.
///
/// The server relays provider events over the local socket; when the upstream
/// model reasons silently, no events cross the socket, so a hardcoded short
/// watchdog cannot distinguish a dead connection from a healthy long think
/// (issue #434). Derive it from `[provider] stream_idle_timeout_secs`, scaled by
/// the largest reasoning-effort multiplier because effort is invisible here,
/// plus grace so the server-side idle timeout (which produces a visible error
/// event) always fires first. Never below 2 minutes.
pub(super) fn stall_timeout() -> Duration {
    const MIN_STALL_TIMEOUT: Duration = Duration::from_secs(2 * 60);
    const GRACE: Duration = Duration::from_secs(30);
    let provider_idle = crate::provider::max_stream_idle_timeout();
    provider_idle.saturating_add(GRACE).max(MIN_STALL_TIMEOUT)
}

/// Human-readable stall duration for user-facing stall messages, e.g.
/// "2 minutes", "3.5 minutes", or "90 seconds".
pub(super) fn format_stall_duration(timeout: Duration) -> String {
    let secs = timeout.as_secs();
    if secs < 120 {
        format!("{} seconds", secs)
    } else if secs.is_multiple_of(60) {
        format!("{} minutes", secs / 60)
    } else {
        format!("{:.1} minutes", secs as f64 / 60.0)
    }
}

pub(super) async fn detect_and_cancel_stall(app: &mut App, remote: &mut RemoteConnection) {
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
            if let Err(error) = remote.cancel_with_reason("stall_guard").await {
                crate::logging::warn(&format!("Stall-guard cancellation could not reach daemon: {error}"));
            }
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
