//! Display-only completion reports. Assessment weaknesses never enqueue work.

use super::App;
use crate::tui::DisplayMessage;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

pub(super) const REPORT_TITLE: &str = "Todo completion";

#[derive(Default)]
pub(super) struct TodoCompletionReportState {
    session_id: Option<String>,
    fingerprint: Option<u64>,
    checked_at: Option<Instant>,
    observations: Vec<crate::todo::GateObservation>,
}

impl App {
    /// Render every finished item and recorded gate finding without entering the
    /// provider conversation or depending on auto-poke being enabled.
    pub(super) fn refresh_completed_todo_report(&mut self, force: bool) -> bool {
        if crate::tui::is_ssh_remote() {
            return false;
        }
        let Some(session_id) = self.active_client_session_id().map(str::to_owned) else {
            return false;
        };
        if self.todo_completion_report.session_id.as_deref() != Some(&session_id) {
            self.todo_completion_report = TodoCompletionReportState {
                session_id: Some(session_id.clone()),
                ..Default::default()
            };
        }
        if !force
            && self
                .todo_completion_report
                .checked_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return false;
        }
        self.todo_completion_report.checked_at = Some(Instant::now());

        let todos = match crate::todo::load_todos(&session_id) {
            Ok(value) => value,
            Err(error) => {
                crate::logging::warn(&format!(
                    "Cannot refresh todo completion report (todos): {error}"
                ));
                // Keep the last valid report instead of declaring unreadable work complete.
                return false;
            }
        };
        if todos.is_empty()
            || todos.iter().any(|todo| {
                !crate::todo::todo_status_is_completed(&todo.status)
                    && !crate::todo::todo_status_is_cancelled(&todo.status)
            })
        {
            // Reopening work starts a new cycle; preserve its earlier report.
            self.todo_completion_report.fingerprint = None;
            self.todo_completion_report.observations.clear();
            return false;
        }
        let plan = match crate::todo::load_plan(&session_id) {
            Ok(value) => value,
            Err(error) => {
                crate::logging::warn(&format!(
                    "Cannot refresh todo completion report (plan): {error}"
                ));
                // Keep the last valid report instead of declaring unreadable work complete.
                return false;
            }
        };
        let goals = match crate::todo::load_goals(&session_id) {
            Ok(value) => value,
            Err(error) => {
                crate::logging::warn(&format!(
                    "Cannot refresh todo completion report (goals): {error}"
                ));
                // Keep the last valid report instead of declaring unreadable work complete.
                return false;
            }
        };
        let recorded = match crate::todo::load_gate_observations(&session_id) {
            Ok(value) => value,
            Err(error) => {
                crate::logging::warn(&format!(
                    "Cannot refresh todo completion report (recorded): {error}"
                ));
                // Keep the last valid report instead of declaring unreadable work complete.
                return false;
            }
        };
        // Turn-end processing clears the live log. Retain this completion
        // cycle's findings, including repeat counts, across that clearing and
        // late assessment writes. A reopened cycle starts a fresh collection.
        for (index, observation) in recorded.iter().enumerate() {
            let count = recorded[..=index]
                .iter()
                .filter(|entry| *entry == observation)
                .count();
            let captured_count = self
                .todo_completion_report
                .observations
                .iter()
                .filter(|entry| *entry == observation)
                .count();
            if count > captured_count {
                self.todo_completion_report
                    .observations
                    .push(observation.clone());
            }
        }
        let observations = &self.todo_completion_report.observations;
        let Ok(serialized) = serde_json::to_vec(&(&todos, &plan, &goals, &observations)) else {
            return false;
        };
        let mut hasher = DefaultHasher::new();
        serialized.hash(&mut hasher);
        let fingerprint = hasher.finish();
        if self.todo_completion_report.fingerprint == Some(fingerprint) {
            return false;
        }

        // Persist data rather than a reference to the live todo file so earlier
        // completion cycles remain unchanged when work resumes.
        let content = serde_json::json!({
            "todo_completion_version": 1,
            "todos": todos,
            "plan": plan,
            "goals": goals,
            "observations": observations,
        })
        .to_string();
        let updating = self.todo_completion_report.fingerprint.is_some();
        self.todo_completion_report.fingerprint = Some(fingerprint);
        // Late evidence/assessment writes update this cycle's report rather
        // than flooding the transcript with a second full copy.
        if updating
            && let Some(index) = self
                .display_messages
                .iter()
                .rposition(|message| message.title.as_deref() == Some(REPORT_TITLE))
        {
            return self.replace_display_message_content(index, content);
        }
        let mut message = DisplayMessage::system(content);
        message.title = Some(REPORT_TITLE.to_string());
        self.push_display_message(message);
        true
    }
}
