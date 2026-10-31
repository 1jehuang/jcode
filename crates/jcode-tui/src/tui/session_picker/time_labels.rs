//! Compact session age and duration labels.

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

/// Compact duration for the "working Ns" badge in the Active view.
pub(crate) fn format_short_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        return format!("{}s", secs);
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{}m", minutes);
    }
    format!("{}h{}m", minutes / 60, minutes % 60)
}
