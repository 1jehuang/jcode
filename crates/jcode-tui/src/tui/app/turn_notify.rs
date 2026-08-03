//! Desktop notification for completed long agent turns.
//!
//! When a turn finishes after a configurable duration (lower threshold when
//! the session has todos, since those indicate task-style work), the user gets
//! a compact desktop notification: session name + duration in the title, todo
//! progress and a short snippet of the final assistant text in the body. By
//! default it fires only while the terminal window is unfocused.

use super::App;
use crate::todo::TodoItem;

/// Maximum characters of assistant text shown in the notification body.
/// Notification banners truncate aggressively; keep the payload tight.
const SNIPPET_MAX_CHARS: usize = 120;

/// Per-todo title clip length for the notification body. Banners are narrow and
/// we may show two todos, so keep each tight.
const TODO_MAX_CHARS: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TurnNotification {
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
}

impl App {
    /// Send a desktop notification for a just-completed turn when warranted.
    /// Call at turn completion, after the final assistant message is committed.
    pub(super) fn maybe_notify_turn_complete(&self, duration_secs: Option<f32>) {
        if !self.runtime_mode_allows_turn_notifications() {
            return;
        }
        let cfg = &crate::config::config().notifications;
        if !cfg.turn_complete {
            return;
        }
        if cfg.turn_complete_only_when_unfocused && self.client_focused() {
            return;
        }
        let Some(duration) = duration_secs else {
            return;
        };

        let todos = self
            .active_client_session_id()
            .map(load_session_todos)
            .unwrap_or_default();
        let threshold = if todos.is_empty() {
            cfg.turn_complete_min_secs
        } else {
            cfg.turn_complete_todo_min_secs
                .min(cfg.turn_complete_min_secs)
        };
        if (duration as u64) < threshold.max(1) {
            return;
        }

        let notification = build_turn_notification(
            self.active_client_session_id()
                .and_then(crate::id::extract_session_name),
            duration,
            &todos,
            self.last_assistant_text_for_notification().as_deref(),
        );
        let sound = cfg.turn_complete_sound.trim();
        let sound = (!sound.is_empty()).then_some(sound);
        crate::notifications::send_desktop_notification_rich(
            &notification.title,
            notification.subtitle.as_deref(),
            &notification.body,
            sound,
        );
    }

    fn runtime_mode_allows_turn_notifications(&self) -> bool {
        matches!(self.runtime_mode(), super::AppRuntimeMode::RemoteClient) && !self.is_replay
    }

    /// Final assistant text of the turn, used for the notification snippet.
    fn last_assistant_text_for_notification(&self) -> Option<String> {
        self.display_messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant" && !m.content.trim().is_empty())
            .map(|m| m.content.clone())
    }
}

fn load_session_todos(session_id: &str) -> Vec<TodoItem> {
    crate::todo::load_todos(session_id).unwrap_or_default()
}

/// Build the compact notification. Kept free of `App` for testability.
///
/// Layout (macOS):
///   title:    jcode · <session> · done in <dur>
///   subtitle: <todo progress, e.g. "3/5 todos · 1 blocked">
///   body:     names the work — "✓ <just done> · → <in progress>" or a
///             blocker ("⊘ <todo> needs <dep>"), falling back to the
///             assistant snippet when there are no todos.
pub(super) fn build_turn_notification(
    session_name: Option<&str>,
    duration_secs: f32,
    todos: &[TodoItem],
    last_assistant_text: Option<&str>,
) -> TurnNotification {
    let mut title = String::from("jcode");
    if let Some(name) = session_name {
        title.push_str(" · ");
        title.push_str(name);
    }
    title.push_str(" · done in ");
    title.push_str(&format_duration_compact(duration_secs));

    let subtitle = todo_progress_line(todos);

    // Prefer naming the actual work; fall back to the assistant snippet.
    let work_line = todo_work_line(todos);
    let snippet = last_assistant_text
        .map(summary_snippet)
        .filter(|s| !s.is_empty());

    let mut body = String::new();
    if let Some(work) = work_line {
        body.push_str(&work);
    } else if let Some(snippet) = snippet {
        body.push_str(&snippet);
    }
    if body.is_empty() {
        body.push_str("Turn finished");
    }

    TurnNotification {
        title,
        subtitle,
        body,
    }
}

/// "3/5 todos" plus "· 1 blocked" when relevant; None when no todos exist.
fn todo_progress_line(todos: &[TodoItem]) -> Option<String> {
    if todos.is_empty() {
        return None;
    }
    let total = todos.len();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let blocked = todos
        .iter()
        .filter(|t| t.status != "completed" && !t.blocked_by.is_empty())
        .count();
    let mut line = if completed == total {
        format!("✓ all {} todos", total)
    } else {
        format!("{}/{} todos", completed, total)
    };
    if blocked > 0 {
        line.push_str(&format!(" · {} blocked", blocked));
    }
    Some(line)
}

/// Names the salient todo work for the body: a blocker if one is the reason the
/// turn stopped, otherwise the most recently completed item and what's next.
/// Returns None when there are no todos (caller falls back to the snippet).
fn todo_work_line(todos: &[TodoItem]) -> Option<String> {
    if todos.is_empty() {
        return None;
    }

    // A blocked, not-yet-done todo is the most actionable thing to surface.
    if let Some(blocked) = todos
        .iter()
        .find(|t| t.status != "completed" && !t.blocked_by.is_empty())
    {
        let dep = blocked
            .blocked_by
            .iter()
            .find_map(|id| resolve_todo_title(todos, id))
            .unwrap_or_else(|| blocked.blocked_by.join(", "));
        return Some(format!(
            "⊘ {} needs {}",
            clip_todo(&blocked.content),
            clip_todo(&dep)
        ));
    }

    let in_progress = todos
        .iter()
        .find(|t| t.status == "in_progress" || t.status == "in-progress");
    let last_done = todos.iter().rev().find(|t| t.status == "completed");

    let mut parts = Vec::new();
    if let Some(done) = last_done {
        let mut seg = format!("✓ {}", clip_todo(&done.content));
        if let Some(conf) = done.completion_confidence
            && conf == crate::todo::ConfidenceState::Speculative
        {
            seg.push_str(&format!(" (low conf: {})", conf.as_str()));
        }
        parts.push(seg);
    }
    if let Some(next) = in_progress {
        parts.push(format!("→ {}", clip_todo(&next.content)));
    } else if last_done.is_none() {
        // Nothing completed and nothing in progress: name the next pending item.
        if let Some(pending) = todos.iter().find(|t| t.status == "pending") {
            parts.push(format!("→ {}", clip_todo(&pending.content)));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn resolve_todo_title(todos: &[TodoItem], id: &str) -> Option<String> {
    todos.iter().find(|t| t.id == id).map(|t| t.content.clone())
}

/// Clip a single todo title for inline display in the notification body.
fn clip_todo(s: &str) -> String {
    let cleaned = strip_markdown_inline(s.trim());
    truncate_chars(cleaned.trim(), TODO_MAX_CHARS)
}

/// First meaningful line of the assistant text, markdown-stripped and clipped.
fn summary_snippet(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty() && !l.starts_with("```") && !l.starts_with('|') && !l.starts_with("---")
        })
        .unwrap_or("");
    let cleaned = strip_markdown_inline(line);
    truncate_chars(cleaned.trim(), SNIPPET_MAX_CHARS)
}

fn strip_markdown_inline(line: &str) -> String {
    let line = line.trim_start_matches('#').trim_start();
    // List/quote markers.
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("> "))
        .unwrap_or(line);
    line.replace("**", "").replace('`', "")
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn format_duration_compact(secs: f32) -> String {
    let secs = secs.max(0.0) as u64;
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{}m", m)
        } else {
            format!("{}m {}s", m, s)
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{}h", h)
        } else {
            format!("{}h {}m", h, m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(status: &str, blocked: bool) -> TodoItem {
        todo_named("x", status, &[]).tap(|t| {
            if blocked {
                t.blocked_by = vec!["other".to_string()];
            }
        })
    }

    fn todo_named(content: &str, status: &str, blocked_by: &[&str]) -> TodoItem {
        TodoItem {
            content: content.to_string(),
            status: status.to_string(),
            priority: "medium".to_string(),
            id: content.to_string(),
            group: None,
            confidence: None,
            completion_confidence: None,
            confidence_history: Vec::new(),
            blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
            assigned_to: None,
        }
    }

    trait Tap: Sized {
        fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }
    impl Tap for TodoItem {}

    #[test]
    fn title_includes_session_and_compact_duration() {
        let n = build_turn_notification(Some("fox"), 754.0, &[], Some("All done."));
        assert_eq!(n.title, "jcode · fox · done in 12m 34s");
        assert_eq!(n.subtitle, None);
        assert_eq!(n.body, "All done.");
    }

    #[test]
    fn subtitle_holds_progress_and_body_names_the_work() {
        let todos = vec![
            todo_named("wire up parser", "completed", &[]),
            todo_named("handle reconnect", "in_progress", &[]),
        ];
        let n = build_turn_notification(None, 200.0, &todos, Some("Fixed the parser bug."));
        assert_eq!(n.title, "jcode · done in 3m 20s");
        assert_eq!(n.subtitle.as_deref(), Some("1/2 todos"));
        // Names actual todo work, not the prose snippet.
        assert_eq!(n.body, "✓ wire up parser · → handle reconnect");
    }

    #[test]
    fn body_names_blocker_and_its_dependency() {
        let todos = vec![
            todo_named("run migration", "pending", &[]),
            todo_named("deploy", "pending", &["run migration"]),
        ];
        let n = build_turn_notification(None, 200.0, &todos, None);
        assert_eq!(n.subtitle.as_deref(), Some("0/2 todos · 1 blocked"));
        assert_eq!(n.body, "⊘ deploy needs run migration");
    }

    #[test]
    fn low_confidence_completion_is_flagged() {
        let mut done = todo_named("risky refactor", "completed", &[]);
        done.completion_confidence = Some(crate::todo::ConfidenceState::from_legacy_score(35));
        let n = build_turn_notification(None, 200.0, &[done], None);
        assert_eq!(n.subtitle.as_deref(), Some("✓ all 1 todos"));
        assert_eq!(n.body, "✓ risky refactor (low conf: speculative)");
    }

    #[test]
    fn all_complete_celebrated_in_subtitle() {
        let done = vec![
            todo_named("a", "completed", &[]),
            todo_named("b", "completed", &[]),
        ];
        let n = build_turn_notification(None, 200.0, &done, None);
        assert_eq!(n.subtitle.as_deref(), Some("✓ all 2 todos"));
        assert_eq!(n.body, "✓ b");
    }

    #[test]
    fn snippet_used_when_no_todos() {
        let n = build_turn_notification(None, 200.0, &[], Some("Fixed the parser bug."));
        assert_eq!(n.subtitle, None);
        assert_eq!(n.body, "Fixed the parser bug.");
    }

    #[test]
    fn snippet_skips_markdown_noise_and_truncates() {
        let text = "```rust\ncode\n```\n\n## **Results** are `good`\nmore detail";
        assert_eq!(summary_snippet(text), "code");

        let text = "\n\n- **Fixed** the `frobnicator`\nrest";
        assert_eq!(summary_snippet(text), "Fixed the frobnicator");

        let long = "a".repeat(300);
        let s = summary_snippet(&long);
        assert_eq!(s.chars().count(), SNIPPET_MAX_CHARS);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn empty_inputs_fall_back_to_minimal_body() {
        let n = build_turn_notification(None, 65.0, &[], None);
        assert_eq!(n.title, "jcode · done in 1m 5s");
        assert_eq!(n.subtitle, None);
        assert_eq!(n.body, "Turn finished");
    }

    #[test]
    fn still_counts_blocked_in_subtitle() {
        let blocked = vec![todo("completed", false), todo("pending", true)];
        let n = build_turn_notification(None, 200.0, &blocked, None);
        assert_eq!(n.subtitle.as_deref(), Some("1/2 todos · 1 blocked"));
    }

    #[test]
    fn duration_formats_hours() {
        assert_eq!(format_duration_compact(59.0), "59s");
        assert_eq!(format_duration_compact(60.0), "1m");
        assert_eq!(format_duration_compact(3600.0), "1h");
        assert_eq!(format_duration_compact(3725.0), "1h 2m");
    }
}
