//! Cheap, session-scoped metadata for the terminal's native tabs.
//!
//! Diff counts are cumulative edit-output lines, matching the transcript's diff
//! badges, not a repository-wide `git diff` (other sessions may share the repo).
//! Time is the current/last observed working episode (including continuations),
//! never the age of an open tab or a fabricated lifetime total.
use super::{App, DisplayMessage};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct TerminalTitleState {
    pub base: String,
    pub session_id: String,
    pub last_sent: String,
    pub last_refresh: Option<Instant>,
    pub last_work: Option<Duration>,
}

pub(super) fn edit_line_counts(message: &DisplayMessage) -> (usize, usize) {
    let Some(tool) = message.tool_data.as_ref().filter(|tool| {
        message.role == "tool"
            && crate::tui::ui::tools_ui::is_edit_tool_name(&tool.name)
            && jcode_tui_tool_display::concise_tool_error_summary(&message.content).is_none()
    }) else {
        return (0, 0);
    };
    crate::tui::ui_diff::diff_change_counts_for_tool(tool, &message.content)
}

fn work_label(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, secs / 60 % 60)
    } else if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn title_with_metrics(
    base: &str,
    counts: (usize, usize),
    work: Option<Duration>,
    active: bool,
) -> String {
    let mut title = base.to_owned();
    if counts != (0, 0) {
        title.push_str(&format!(" · +{} -{}", counts.0, counts.1));
    }
    if let Some(work) = work {
        title.push_str(&format!(
            " · {} ~{}",
            if active { "work" } else { "last" },
            work_label(work)
        ));
    } else if active {
        title.push_str(" · working");
    }
    title
}

impl App {
    /// Tell herdr (when running in a herdr pane) whether this client is
    /// working or idle and which session to resume. Tick-safe: unchanged
    /// state is a cheap comparison and reports are sent off-thread.
    pub(super) fn sync_herdr_agent_state(&self) {
        if self.suppress_terminal_title_updates || !self.is_remote_client() {
            return;
        }
        let state = if self.herdr_core_blocked_kind().is_some() {
            crate::tui::herdr::AgentState::Blocked
        } else if self.is_processing() {
            crate::tui::herdr::AgentState::Working
        } else {
            crate::tui::herdr::AgentState::Idle
        };
        crate::tui::herdr::sync(state, self.active_client_session_id());
        self.sync_herdr_core_agent_state();
    }

    /// Report state to Herdr Desktop (herdr-core) over OSC 7501. Runs on the
    /// UI thread between frames; a no-op outside herdr-core terminals.
    fn sync_herdr_core_agent_state(&self) {
        if !crate::tui::herdr_core::enabled() {
            return;
        }
        let state = if let Some(kind) = self.herdr_core_blocked_kind() {
            crate::tui::herdr_core::State::Blocked(kind)
        } else if self.is_processing() {
            crate::tui::herdr_core::State::Working
        } else {
            crate::tui::herdr_core::State::Idle
        };
        let title = self.terminal_title.borrow().base.clone();
        let activity = self.herdr_core_activity(state);
        crate::tui::herdr_core::sync(state, &title, &activity);
    }

    /// One short line for herdr's `msg`: what jcode is doing or waiting on.
    /// Deliberately free of timers and token counts, so it only changes
    /// when the activity does and reports stay rare.
    fn herdr_core_activity(&self, state: crate::tui::herdr_core::State) -> String {
        use super::ProcessingStatus;
        use crate::tui::herdr_core::{BlockedKind, State};
        match state {
            State::Idle => return String::new(),
            State::Blocked(BlockedKind::Auth) => return "Waiting for login input".into(),
            State::Blocked(BlockedKind::Permission) => return "Waiting for approval".into(),
            State::Blocked(BlockedKind::Question) => {
                let key = crate::tui::keybind::fallback_switch_key_label();
                return match &self.pending_fallback_offer {
                    Some(offer) => format!(
                        "{} failed. Press {} to switch to {}",
                        offer.from_label, key, offer.target_label
                    ),
                    None => "Waiting for you".into(),
                };
            }
            State::Working => {}
        }
        if let Some(subagent) = self.subagent_status().filter(|s| !s.trim().is_empty()) {
            return subagent.to_string();
        }
        // An in-flight tool call is the most specific answer. Remote turns
        // may keep `status` at Streaming while a tool runs, so look at the
        // calls themselves rather than only at `RunningTool`.
        let running_tool = match &self.status {
            ProcessingStatus::RunningTool(name) => Some(name.as_str()),
            _ => self
                .streaming_tool_calls
                .last()
                .map(|call| call.name.as_str()),
        };
        if let Some(name) = running_tool {
            return self.herdr_core_tool_activity(name);
        }
        match &self.status {
            ProcessingStatus::RunningTool(name) => self.herdr_core_tool_activity(name),
            ProcessingStatus::Thinking(_) => "Thinking".into(),
            ProcessingStatus::Streaming => "Writing".into(),
            ProcessingStatus::Sending => "Sending".into(),
            ProcessingStatus::Connecting(phase) => format!("Connecting: {phase}"),
            ProcessingStatus::WaitingForNetwork { .. } => "Waiting for network".into(),
            ProcessingStatus::Idle => "Working".into(),
        }
    }

    /// `bash: Run the tests`, `Running batch: 2/5 done`, or `Running grep`.
    fn herdr_core_tool_activity(&self, name: &str) -> String {
        if name == "batch"
            && let Some(progress) = self.batch_progress.as_ref()
        {
            return format!(
                "Running batch: {}/{} done",
                progress.completed, progress.total
            );
        }
        let detail = self
            .streaming_tool_calls
            .iter()
            .rev()
            .find(|call| call.name == name)
            .map(crate::tui::ui::tools_ui::get_tool_activity_detail)
            .filter(|detail| !detail.trim().is_empty());
        match detail {
            Some(detail) => format!("{name}: {detail}"),
            None => format!("Running {name}"),
        }
    }

    /// Whether jcode is stopped until the user acts, and why. A pending login
    /// waits for a pasted code or key; a fallback offer waits for the switch
    /// key after a failed turn. The automatic failover countdown proceeds on
    /// its own, so it is not blocked.
    fn herdr_core_blocked_kind(&self) -> Option<crate::tui::herdr_core::BlockedKind> {
        use crate::tui::herdr_core::BlockedKind;
        if self
            .pending_login
            .as_ref()
            .is_some_and(|login| !matches!(login, super::PendingLogin::Exchanging { .. }))
        {
            return Some(BlockedKind::Auth);
        }
        if self.pending_fallback_offer.is_some() && !self.is_processing() {
            return Some(BlockedKind::Question);
        }
        None
    }

    pub(super) fn set_terminal_title_base(&self, session_id: &str, base: String) {
        let mut state = self.terminal_title.borrow_mut();
        if state.session_id != session_id {
            *state = TerminalTitleState::default();
            state.session_id = session_id.to_owned();
        }
        state.base = base;
        state.last_refresh = None;
        drop(state);
        self.refresh_terminal_title_metrics();
    }

    /// Tick-safe: no filesystem access, transcript scans, or subprocesses.
    /// At most one OSC title update per second, and only if the text changed.
    pub(super) fn refresh_terminal_title_metrics(&self) {
        if self.suppress_terminal_title_updates {
            return;
        }
        let mut state = self.terminal_title.borrow_mut();
        if state.base.is_empty()
            || state
                .last_refresh
                .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        state.last_refresh = Some(Instant::now());
        let active = self.is_processing();
        let work = if active {
            self.display_turn_duration_secs()
                .filter(|secs| secs.is_finite() && *secs >= 0.0)
                .map(Duration::from_secs_f32)
        } else {
            state.last_work
        };
        if active && work.is_some() {
            state.last_work = work;
        }
        let title = title_with_metrics(&state.base, self.display_edit_line_counts, work, active);
        if state.last_sent != title
            && crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(&title)).is_ok()
        {
            state.last_sent = title;
        }
    }

    pub(super) fn remember_terminal_title_work(&self) {
        if let Some(secs) = self
            .display_turn_duration_secs()
            .filter(|secs| secs.is_finite() && *secs >= 0.0)
        {
            let mut state = self.terminal_title.borrow_mut();
            state.last_work = Some(Duration::from_secs_f32(secs));
            state.last_refresh = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolCall;

    #[test]
    fn terminal_title_counts_follow_append_remove_and_history_replace() {
        let mut app = super::super::tests::create_test_app();
        app.suppress_terminal_title_updates = true;
        let message = DisplayMessage::tool(
            "@@ -1 +1 @@\n-old\n+new\n+extra",
            ToolCall {
                id: "edit-1".into(),
                name: "edit".into(),
                input: serde_json::json!({"file_path": "example.rs"}),
                intent: None,
                thought_signature: None,
            },
        );
        app.push_display_message(message.clone());
        assert_eq!(app.display_edit_line_counts, (2, 1));
        app.push_display_message(DisplayMessage::user("continue"));
        assert_eq!(app.display_edit_line_counts, (2, 1));
        app.adjust_display_message_stats(&message, false);
        assert_eq!(app.display_edit_line_counts, (0, 0));
        app.replace_display_messages(vec![message.clone(), message]);
        assert_eq!(app.display_edit_line_counts, (4, 2));
        app.replace_display_messages(vec![]);
        assert_eq!(app.display_edit_line_counts, (0, 0));
    }

    #[test]
    fn terminal_title_work_freezes_and_resets_on_session_switch() {
        let mut app = super::super::tests::create_test_app();
        app.suppress_terminal_title_updates = true;
        app.set_terminal_title_base("first", "First task".into());
        app.visible_turn_started = Some(Instant::now() - Duration::from_secs(125));
        app.clear_visible_turn_started();
        let completed = app.terminal_title.borrow().last_work.unwrap();
        assert_eq!(completed.as_secs(), 125);
        app.remember_terminal_title_work();
        assert_eq!(app.terminal_title.borrow().last_work, Some(completed));
        app.set_terminal_title_base("first", "Renamed task".into());
        assert_eq!(app.terminal_title.borrow().last_work, Some(completed));
        app.set_terminal_title_base("second", "Second task".into());
        assert_eq!(app.terminal_title.borrow().last_work, None);
        assert!(app.terminal_title.borrow().last_sent.is_empty());
    }

    #[test]
    fn title_metrics_show_edits_and_current_or_last_turn() {
        let base = "🦊 Improve session tabs";
        assert_eq!(title_with_metrics(base, (0, 0), None, false), base);
        assert_eq!(
            title_with_metrics(base, (12, 3), Some(Duration::from_secs(125)), true),
            "🦊 Improve session tabs · +12 -3 · work ~2m05s"
        );
        assert_eq!(
            title_with_metrics(base, (0, 4), Some(Duration::from_secs(125)), false),
            "🦊 Improve session tabs · +0 -4 · last ~2m05s"
        );
        assert_eq!(
            title_with_metrics(base, (0, 0), None, true),
            "🦊 Improve session tabs · working"
        );
    }

    #[test]
    fn work_duration_boundaries_are_compact() {
        for (secs, expected) in [
            (0, "0s"),
            (59, "59s"),
            (60, "1m00s"),
            (3599, "59m59s"),
            (3600, "1h00m"),
            (90061, "25h01m"),
        ] {
            assert_eq!(work_label(Duration::from_secs(secs)), expected);
        }
    }

    #[test]
    fn edit_counts_ignore_failures_and_non_edit_output() {
        let mut message = DisplayMessage::tool(
            "@@ -1 +1 @@\n-old\n+new\n+extra",
            ToolCall {
                id: "edit-1".into(),
                name: "edit".into(),
                input: serde_json::json!({"file_path": "example.rs"}),
                intent: None,
                thought_signature: None,
            },
        );
        assert_eq!(edit_line_counts(&message), (2, 1));
        message.tool_data.as_mut().unwrap().name = "bash".into();
        assert_eq!(edit_line_counts(&message), (0, 0));
        message.tool_data.as_mut().unwrap().name = "edit".into();
        message.content = "Error: could not apply patch\n-old\n+new".into();
        assert_eq!(edit_line_counts(&message), (0, 0));
    }
}
