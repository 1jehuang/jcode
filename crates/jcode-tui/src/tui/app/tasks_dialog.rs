//! Glue for the background tasks dialog: the footer pill's ↓ selection, the
//! data snapshot (refreshed about once a second while open), entering an
//! agent and stopping a task.

use super::*;
use crate::tui::TuiState;
use crate::tui::backend::RemoteConnection;
use crate::tui::dialog::{
    Dialog, ShellTask, TAIL_LINES, TaskTarget, TasksSnapshot, read_output_tail, task_pill,
};

impl App {
    /// Shell rows come from this session's task events; runtime and output
    /// come from the task's status and output files, which the daemon writes
    /// on this machine.
    fn tasks_snapshot(&self) -> TasksSnapshot {
        let manager = crate::background::global();
        let shells = self
            .background_task_rows
            .iter()
            .map(|row| {
                // Task IDs arrive from the server; never let one name a path.
                let plain_id = std::path::Path::new(&row.task_id)
                    .file_name()
                    .and_then(|name| name.to_str())
                    == Some(row.task_id.as_str());
                let status: Option<crate::background::TaskStatusFile> = plain_id
                    .then(|| std::fs::read(manager.status_path_for(&row.task_id)).ok())
                    .flatten()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                let running = row.status == crate::tui::BackgroundTaskRowStatus::Running;
                let runtime_secs = status
                    .as_ref()
                    .and_then(|status| match status.duration_secs {
                        Some(secs) if !running => Some(secs as u64),
                        _ => chrono::DateTime::parse_from_rfc3339(&status.started_at)
                            .ok()
                            .map(|start| {
                                (chrono::Utc::now() - start.with_timezone(&chrono::Utc))
                                    .num_seconds()
                                    .max(0) as u64
                            }),
                    });
                ShellTask {
                    task_id: row.task_id.clone(),
                    label: row.label.clone(),
                    status: row.status,
                    runtime_secs,
                    exit_code: status.as_ref().and_then(|status| status.exit_code),
                    error: status.and_then(|status| status.error),
                    output: plain_id
                        .then(|| {
                            read_output_tail(&manager.output_path_for(&row.task_id), TAIL_LINES)
                        })
                        .flatten(),
                }
            })
            .collect();
        TasksSnapshot {
            shells,
            agents: self.inline_swarm_members(),
            taken_at: Some(Instant::now()),
        }
    }

    pub(super) fn open_tasks_dialog(&mut self) {
        self.task_pill_selected = false;
        self.dialog = Some(Dialog::tasks(self.tasks_snapshot()));
    }

    /// Called on every tick; re-reads task state once a second while open.
    pub(super) fn refresh_tasks_dialog(&mut self) -> bool {
        if !self.dialog.as_ref().is_some_and(Dialog::tasks_refresh_due) {
            return false;
        }
        let snapshot = self.tasks_snapshot();
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.set_tasks(snapshot);
        }
        true
    }

    /// Claude Code's ↓ on an empty prompt: the first press selects the footer
    /// pill, a second ↓ or Enter opens the dialog, Esc or ↑ deselects, and any
    /// other key deselects and keeps its usual meaning.
    pub(super) fn handle_task_pill_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        // A paste or finished tasks leave nothing to open: the key acts as usual.
        if std::mem::take(&mut self.task_pill_selected) && self.task_pill_selectable() {
            return match code {
                KeyCode::Down | KeyCode::Enter if modifiers.is_empty() => {
                    self.open_tasks_dialog();
                    true
                }
                KeyCode::Esc | KeyCode::Up => true,
                _ => false,
            };
        }
        // ↓ keeps scrolling a chat that is not at the bottom or a focused pane.
        if code != KeyCode::Down
            || !modifiers.is_empty()
            || self.auto_scroll_paused
            || self.pending_history_anchor.is_some()
            || self.inline_interactive_state.is_some()
            || self.diff_pane_focus
            || (self.diagram_focus && self.diagram_available())
        {
            return false;
        }
        self.task_pill_selected = self.task_pill_selectable();
        self.task_pill_selected
    }

    /// The pill can be selected only on an empty prompt while tasks run.
    pub(super) fn task_pill_selectable(&self) -> bool {
        self.input.is_empty()
            && task_pill(&self.background_task_rows, &self.inline_swarm_members()).is_some()
    }

    /// Switch to a spawned agent through the swarm panel's enter path.
    pub(super) fn enter_swarm_agent(&mut self, session_id: &str) {
        let members = self.inline_swarm_members();
        let order = crate::tui::info_widget::swarm_gallery::members_display_order(&members);
        match order.iter().position(|id| id == session_id) {
            Some(index) => {
                self.swarm_panel_selected = index;
                self.enter_selected_swarm_agent();
            }
            None => self.set_status_notice("That agent is no longer in this swarm"),
        }
    }

    /// Without a daemon only local-mode shells can be stopped: they run in
    /// this process.
    pub(super) fn stop_task_offline(&mut self, target: TaskTarget) {
        match target {
            TaskTarget::Shell(task_id) if !self.is_remote => {
                tokio::spawn(async move {
                    if let Err(error) = crate::background::global().cancel(&task_id).await {
                        crate::logging::warn(&format!(
                            "Failed to stop background task {task_id}: {error}"
                        ));
                    }
                });
            }
            _ => {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.set_status(Some("Not connected to the jcode server".to_string()));
                }
            }
        }
    }
}

/// Shells are cancelled by the daemon's background manager, agents through
/// the swarm stop request the `swarm` tool uses.
pub(super) async fn stop_task_remote(
    app: &mut App,
    remote: &mut RemoteConnection,
    target: TaskTarget,
) {
    let sent = match target {
        TaskTarget::Shell(task_id) => remote
            .runtime_command("background_cancel", vec![task_id])
            .await
            .map(|_| ()),
        TaskTarget::Agent(session_id) => remote.stop_swarm_member(session_id).await,
    };
    if let Err(error) = sent
        && let Some(dialog) = app.dialog.as_mut()
    {
        dialog.set_status(Some(format!("Could not stop: {error}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_running_shell() -> App {
        let mut app = App::new_for_remote(None);
        app.upsert_running_background_task("bg-pill-test".into(), "sleep 40".into(), None);
        app
    }

    #[test]
    fn down_selects_the_pill_then_opens_the_dialog() {
        let mut app = app_with_running_shell();
        let none = KeyModifiers::NONE;
        assert!(app.handle_task_pill_key(KeyCode::Down, none));
        assert!(app.task_pill_selected);
        assert!(app.handle_task_pill_key(KeyCode::Down, none));
        assert!(!app.task_pill_selected);
        assert!(app.dialog.as_ref().is_some_and(Dialog::is_tasks));

        // Enter also opens; any other key deselects and keeps its meaning.
        let mut app = app_with_running_shell();
        assert!(app.handle_task_pill_key(KeyCode::Down, none));
        assert!(app.handle_task_pill_key(KeyCode::Enter, none));
        assert!(app.dialog.is_some());
        let mut app = app_with_running_shell();
        assert!(app.handle_task_pill_key(KeyCode::Down, none));
        assert!(!app.handle_task_pill_key(KeyCode::Char('a'), none));
        assert!(!app.task_pill_selected && app.dialog.is_none());
    }

    #[test]
    fn down_keeps_its_meaning_without_tasks_a_draft_or_at_scrollback() {
        let none = KeyModifiers::NONE;
        let mut app = App::new_for_remote(None);
        assert!(!app.handle_task_pill_key(KeyCode::Down, none));
        let mut app = app_with_running_shell();
        app.input = "draft".into();
        assert!(!app.handle_task_pill_key(KeyCode::Down, none));
        let mut app = app_with_running_shell();
        app.auto_scroll_paused = true;
        assert!(!app.handle_task_pill_key(KeyCode::Down, none));
        assert!(!app.task_pill_selected);
    }

    #[test]
    fn down_scrolls_a_focused_pane_instead_of_selecting_the_pill() {
        let mut app = app_with_running_shell();
        app.diff_pane_focus = true;
        assert!(!app.handle_task_pill_key(KeyCode::Down, KeyModifiers::NONE));
        assert!(!app.task_pill_selected);
    }

    #[test]
    fn enter_after_a_paste_or_when_tasks_end_does_not_open_the_dialog() {
        let none = KeyModifiers::NONE;
        let mut app = app_with_running_shell();
        assert!(app.handle_task_pill_key(KeyCode::Down, none));
        app.input = "pasted text".into();
        assert!(!crate::tui::TuiState::task_pill_selected(&app));
        assert!(!app.handle_task_pill_key(KeyCode::Enter, none));
        assert!(app.dialog.is_none());

        let mut app = app_with_running_shell();
        assert!(app.handle_task_pill_key(KeyCode::Down, none));
        app.background_task_rows.clear();
        assert!(!crate::tui::TuiState::task_pill_selected(&app));
        assert!(!app.handle_task_pill_key(KeyCode::Enter, none));
        assert!(app.dialog.is_none());
    }
}
