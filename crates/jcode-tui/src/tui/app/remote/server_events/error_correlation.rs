use super::*;

pub(super) fn has_resumed_turn_evidence(app: &App, had_remote_resume_activity: bool) -> bool {
    had_remote_resume_activity
        || app.stream_message_ended
        || app.has_streaming_footer_stats()
        || !app.streaming.streaming_text.is_empty()
        || !app.streaming_tool_calls.is_empty()
        || matches!(
            app.status,
            ProcessingStatus::Streaming | ProcessingStatus::RunningTool(_)
        )
}

pub(super) fn handle_unrelated_error(
    app: &mut App,
    event: &ServerEvent,
    remote: &mut impl RemoteEventState,
) -> bool {
    let ServerEvent::Error { id, message, .. } = event else {
        return false;
    };
    // Control requests and stale turns also produce Error frames. They must
    // not clear an active turn or stop its waiting retries. Attached clients
    // can observe a turn owned by another connection (or server id 0), but an
    // error for a request issued here needs to match our current message.
    // An adopted turn may fail during connection setup, before any output.
    let has_turn_evidence = has_resumed_turn_evidence(app, app.remote_resume_activity.is_some())
        || app.status_detail.is_some()
        || matches!(app.status, ProcessingStatus::Connecting(_));
    let fails_resumed_turn = app.current_message_id.is_none()
        && app.is_processing
        && has_turn_evidence
        && !remote.issued_request_id(*id);
    if app.current_message_id == Some(*id) || fails_resumed_turn {
        return false;
    }
    if remote.finish_session_launch(*id) && !has_turn_evidence {
        finish_remote_split_launch(app);
    }
    app.push_display_message(DisplayMessage::error(message.clone()));
    true
}
