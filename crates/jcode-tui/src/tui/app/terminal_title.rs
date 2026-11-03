//! Cheap, session-scoped metadata for the terminal's native tabs.
//!
//! Diff counts are cumulative edit-output lines, matching the transcript's diff
//! badges, not a repository-wide `git diff` (other sessions may share the repo).
//! Time is the current/last observed working episode (including continuations),
//! never the age of an open tab or a fabricated lifetime total.
use super::{App, DisplayMessage};
use crate::tui::program_status::{self, ProgramState};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum TurnOutcome {
    #[default]
    None,
    Done,
    Error,
    Interrupted,
}

fn program_state(processing: bool, outcome: TurnOutcome, guardrail_stopped: bool) -> ProgramState {
    if processing {
        return ProgramState::Working;
    }
    match outcome {
        TurnOutcome::None | TurnOutcome::Interrupted => ProgramState::Idle,
        TurnOutcome::Done if guardrail_stopped => ProgramState::Error,
        TurnOutcome::Done => ProgramState::Done,
        TurnOutcome::Error => ProgramState::Error,
    }
}

#[derive(Default)]
pub(super) struct TerminalTitleState {
    pub base: String,
    pub session_id: String,
    pub last_sent: String,
    pub last_refresh: Option<Instant>,
    pub last_work: Option<Duration>,
    pub turn_outcome: TurnOutcome,
    pub last_program_state: Option<ProgramState>,
    pub last_program_message: String,
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
        let state = if self.is_processing() {
            crate::tui::herdr::AgentState::Working
        } else {
            crate::tui::herdr::AgentState::Idle
        };
        crate::tui::herdr::sync(state, self.active_client_session_id());
        self.sync_program_status();
    }

    /// Record how the last turn ended so the OSC 7501 reporter can say
    /// `done`, `error`, or `idle` instead of guessing from the screen.
    pub(super) fn note_turn_done(&self) {
        self.terminal_title.borrow_mut().turn_outcome = TurnOutcome::Done;
    }

    pub(super) fn note_turn_failed(&self) {
        self.terminal_title.borrow_mut().turn_outcome = TurnOutcome::Error;
    }

    pub(super) fn note_turn_interrupted(&self) {
        self.terminal_title.borrow_mut().turn_outcome = TurnOutcome::Interrupted;
    }

    /// Emit the current state through OSC 7501 when it changes. The TUI
    /// client owns the pane PTY, so this is the program-to-terminal channel
    /// herdr-core reads. Cheap enough for every tick: unchanged reports are
    /// dropped before allocating.
    pub(super) fn sync_program_status(&self) {
        if self.suppress_terminal_title_updates {
            return;
        }
        let (outcome, last_state, last_message) = {
            let state = self.terminal_title.borrow();
            (
                state.turn_outcome,
                state.last_program_state,
                state.last_program_message.clone(),
            )
        };
        let program_state =
            program_state(self.is_processing(), outcome, self.turn_guardrail_stopped);
        let message = if program_state == ProgramState::Error {
            self.last_stream_error
                .clone()
                .or_else(|| {
                    self.turn_guardrail_stopped
                        .then(|| "guardrail stop".to_string())
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        if last_state == Some(program_state) && last_message == message {
            return;
        }
        program_status::emit(&program_status::report(
            program_state,
            (!message.is_empty()).then_some(message.as_str()),
        ));
        let mut state = self.terminal_title.borrow_mut();
        state.last_program_state = Some(program_state);
        state.last_program_message = message;
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

    #[test]
    fn program_status_tracks_turn_lifecycle() {
        assert_eq!(
            program_state(true, TurnOutcome::Error, false),
            ProgramState::Working
        );
        assert_eq!(
            program_state(false, TurnOutcome::None, false),
            ProgramState::Idle
        );
        assert_eq!(
            program_state(false, TurnOutcome::Interrupted, false),
            ProgramState::Idle
        );
        assert_eq!(
            program_state(false, TurnOutcome::Done, false),
            ProgramState::Done
        );
        assert_eq!(
            program_state(false, TurnOutcome::Done, true),
            ProgramState::Error
        );
        assert_eq!(
            program_state(false, TurnOutcome::Error, false),
            ProgramState::Error
        );
    }
}
