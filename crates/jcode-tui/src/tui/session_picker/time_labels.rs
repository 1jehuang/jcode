//! Compact session age labels.

use crate::config::ResumeTimeMode;
use jcode_tui_session_picker::SessionInfo;

/// Timestamp Resume shows and sorts by, per `display.resume_time`.
pub(crate) fn session_resume_time(session: &SessionInfo) -> chrono::DateTime<chrono::Utc> {
    session_resume_time_for(session, crate::config::config().display.resume_time)
}

pub(crate) fn session_resume_time_for(
    session: &SessionInfo,
    mode: ResumeTimeMode,
) -> chrono::DateTime<chrono::Utc> {
    let message = session.last_message_time;
    match mode {
        ResumeTimeMode::Message => message,
        ResumeTimeMode::Activity => session.last_active_at.map_or(message, |a| a.max(message)),
        ResumeTimeMode::Opened => session.last_active_at.unwrap_or(message),
        ResumeTimeMode::Created => session.created_at,
    }
}

pub(crate) fn format_time_ago(time: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(time);

    let seconds = duration.num_seconds();
    if seconds < 60 {
        return format!("{}s ago", seconds);
    }

    let minutes = duration.num_minutes();
    if minutes < 60 {
        return format!("{}m ago", minutes);
    }

    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{}h ago", hours);
    }

    let days = duration.num_days();
    if days < 7 {
        return format!("{}d ago", days);
    }

    if days < 30 {
        return format!("{}w ago", days / 7);
    }

    format!("{}mo ago", days / 30)
}
