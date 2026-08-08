//! Parsing background-task notifications into a typed progress event.
//!
//! The daemon's only channel for background work is a `notification` carrying
//! the markdown its own UI renders (`format_background_task_progress_markdown`
//! and `format_background_task_notification_markdown` in `jcode-base`). This
//! module reads the two facts an API client actually needs out of that prose:
//! which task, and how far along.
//!
//! Kept as a hand-written scanner rather than a dependency on the app core:
//! the bridge is a thin translation layer, and pulling the whole application
//! foundation in to read one header line would make it anything but.

use jcode_harness_api::ApiEvent;

/// A background task's state, as far as a notification revealed it.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundProgress {
    pub session_id: String,
    pub task_id: String,
    pub label: String,
    pub percent: Option<f32>,
    pub summary: String,
    pub done: bool,
}

impl BackgroundProgress {
    pub fn into_event(self) -> ApiEvent {
        ApiEvent::BackgroundProgress {
            session_id: self.session_id,
            task_id: self.task_id,
            label: self.label,
            percent: self.percent,
            summary: self.summary,
            done: self.done,
        }
    }
}

/// Read a background-task progress tick or completion out of a notification
/// body, or `None` when the notification is about something else (a DM, a file
/// conflict, shared context).
pub fn parse_background_notification(message: &str) -> Option<BackgroundProgress> {
    let normalized = message.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    let mut lines = trimmed.lines();
    let header = lines.next()?.trim();

    // Order matters: the progress header is a prefix-extension of the
    // completion header, so the longer marker is tested first.
    if let Some(rest) = header.strip_prefix("**Background task progress** ") {
        let (task_id, rest) = split_backticked(rest)?;
        let label = header_label(rest);
        // The detail is either inline after another ` · ` or on the body line.
        let detail = inline_detail(rest).map(str::to_string).or_else(|| {
            let body = lines
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            (!body.is_empty()).then_some(body)
        })?;
        let summary = clean_summary(&detail);
        return Some(BackgroundProgress {
            session_id: String::new(),
            task_id,
            label,
            percent: parse_percent(&summary),
            summary,
            done: false,
        });
    }

    if let Some(rest) = header.strip_prefix("**Background task** ") {
        let (task_id, rest) = split_backticked(rest)?;
        let label = header_label(rest);
        // The completion header is `label · status · 1.2s · exit 0`; the status
        // and duration are the whole story for a finished task.
        let summary = rest
            .split(" · ")
            .skip(1)
            .collect::<Vec<_>>()
            .join(" · ")
            .trim()
            .to_string();
        let summary = if summary.is_empty() {
            "finished".to_string()
        } else {
            summary
        };
        return Some(BackgroundProgress {
            session_id: String::new(),
            task_id,
            label,
            percent: None,
            summary,
            done: true,
        });
    }

    None
}

/// Split a leading `` `task id` `` off the header, returning it and the rest.
fn split_backticked(rest: &str) -> Option<(String, &str)> {
    let rest = rest.strip_prefix('`')?;
    let (id, tail) = rest.split_once('`')?;
    if id.trim().is_empty() {
        return None;
    }
    Some((
        id.trim().to_string(),
        tail.trim_start().trim_start_matches('·').trim_start(),
    ))
}

/// The task's human label: the display name when the header carries one
/// (`` `Model list refresh` (`catalog`) ``), else the tool name.
fn header_label(rest: &str) -> String {
    let first = rest.split(" · ").next().unwrap_or(rest).trim();
    let mut parts = first.split('`').filter(|part| !part.trim().is_empty());
    parts
        .next()
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "background task".to_string())
}

/// A progress detail written on the header line rather than the body.
fn inline_detail(rest: &str) -> Option<&str> {
    let (_, detail) = rest.split_once(" · ")?;
    let detail = detail.trim();
    (!detail.is_empty()).then_some(detail)
}

/// Strip the ASCII bar and the `(reported)` provenance suffix: a client draws
/// its own bar, and where the number came from is not a status line.
fn clean_summary(detail: &str) -> String {
    let mut summary = detail.trim();
    if let Some((bar, rest)) = summary.split_once("] ")
        && bar.starts_with('[')
        && bar[1..].chars().all(|ch| matches!(ch, '#' | '-'))
    {
        summary = rest.trim();
    }
    for source in ["reported", "parsed", "estimated"] {
        if let Some(stripped) = summary.strip_suffix(&format!(" ({source})")) {
            summary = stripped.trim();
            break;
        }
    }
    summary.to_string()
}

/// The first percentage in a summary, e.g. `42%` in `42% · Running tests`.
fn parse_percent(summary: &str) -> Option<f32> {
    let index = summary.find('%')?;
    let digits: String = summary[..index]
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    let digits: String = digits.chars().rev().collect();
    digits
        .parse::<f32>()
        .ok()
        .map(|percent| percent.clamp(0.0, 100.0))
}

#[cfg(test)]
#[path = "background_progress_tests.rs"]
mod tests;
