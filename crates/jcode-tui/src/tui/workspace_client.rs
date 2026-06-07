use crate::session::{Session, SessionStatus};
use crate::tui::workspace_map::{
    VisibleWorkspaceRow, WorkspaceMapModel, WorkspaceSessionTile, WorkspaceSessionVisualState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSplitTarget {
    Right,
    Up,
    Down,
}

/// Per-client workspace navigation state.
///
/// Previously stored in a process-global `Mutex<Option<...>>`; now owned by
/// [`crate::tui::app::App`] so each client instance carries its own workspace
/// map instead of sharing one across the process.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceClientState {
    enabled: bool,
    map: WorkspaceMapModel,
    imported_server_sessions: bool,
    pending_split_target: Option<WorkspaceSplitTarget>,
    pending_resume_session: Option<String>,
}

impl WorkspaceClientState {
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn enable(&mut self, current_session_id: Option<&str>, all_sessions: &[String]) {
        self.enabled = true;
        if self.map.is_empty() {
            self.import_initial_row(current_session_id, all_sessions);
        } else if let Some(session_id) = current_session_id {
            let _ = self.map.focus_session_by_id(session_id);
        }
    }

    pub(crate) fn disable(&mut self) {
        self.enabled = false;
        self.pending_split_target = None;
        self.pending_resume_session = None;
    }

    #[cfg(test)]
    pub(crate) fn reset_for_tests(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn status_summary(&self) -> String {
        if !self.enabled {
            return "Workspace mode: off".to_string();
        }
        let rows = self.map.visible_rows(5);
        let populated = self.map.populated_workspaces().len();
        let total_sessions: usize = rows.iter().map(|row| row.sessions.len()).sum();
        format!(
            "Workspace mode: on\nCurrent workspace: {}\nVisible rows: {}\nPopulated workspaces: {}\nMapped sessions: {}",
            self.map.current_workspace(),
            rows.len(),
            populated,
            total_sessions
        )
    }

    pub(crate) fn sync_after_history(&mut self, current_session_id: &str, all_sessions: &[String]) {
        if !self.enabled {
            return;
        }
        if self.map.is_empty() {
            self.import_initial_row(Some(current_session_id), all_sessions);
            return;
        }
        if self.map.focus_session_by_id(current_session_id) {
            return;
        }
        let tile = WorkspaceSessionTile::new(current_session_id.to_string());
        let _ = self.map.add_session_to_current_workspace(tile);
    }

    pub(crate) fn queue_split_target(&mut self, target: WorkspaceSplitTarget) {
        self.enabled = true;
        self.pending_split_target = Some(target);
    }

    pub(crate) fn take_pending_resume_session(&mut self) -> Option<String> {
        self.pending_resume_session.take()
    }

    pub(crate) fn queue_resume_session(&mut self, session_id: String) {
        self.pending_resume_session = Some(session_id);
    }

    pub(crate) fn handle_split_response(&mut self, new_session_id: &str) -> bool {
        if !self.enabled || self.pending_split_target.is_none() {
            self.pending_split_target = None;
            return false;
        }
        let target = self
            .pending_split_target
            .take()
            .unwrap_or(WorkspaceSplitTarget::Right);
        let target_workspace = match target {
            WorkspaceSplitTarget::Right => self.map.current_workspace(),
            WorkspaceSplitTarget::Up => self.map.current_workspace() + 1,
            WorkspaceSplitTarget::Down => self.map.current_workspace() - 1,
        };
        let _ = self.map.insert_session_in_workspace(
            target_workspace,
            WorkspaceSessionTile::new(new_session_id.to_string()),
        );
        let _ = self.map.focus_session_by_id(new_session_id);
        self.pending_resume_session = Some(new_session_id.to_string());
        true
    }

    pub(crate) fn navigate_left(&mut self) -> Option<String> {
        if !self.enabled || !self.map.move_left() {
            return None;
        }
        self.map
            .current_focused_session_id()
            .map(ToString::to_string)
    }

    pub(crate) fn navigate_right(&mut self) -> Option<String> {
        if !self.enabled || !self.map.move_right() {
            return None;
        }
        self.map
            .current_focused_session_id()
            .map(ToString::to_string)
    }

    pub(crate) fn navigate_up(&mut self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let target_workspace = self.map.nearest_populated_workspace_above()?;
        self.map.set_current_workspace(target_workspace);
        self.map
            .focused_session_in_workspace(target_workspace)
            .map(ToString::to_string)
    }

    pub(crate) fn navigate_down(&mut self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let target_workspace = self.map.nearest_populated_workspace_below()?;
        self.map.set_current_workspace(target_workspace);
        self.map
            .focused_session_in_workspace(target_workspace)
            .map(ToString::to_string)
    }

    pub(crate) fn visible_rows(
        &self,
        max_rows: usize,
        current_session_id: Option<&str>,
        current_session_running: bool,
    ) -> Vec<VisibleWorkspaceRow> {
        let mut rows = if self.enabled {
            self.map.visible_rows(max_rows)
        } else {
            Vec::new()
        };
        for row in &mut rows {
            for tile in &mut row.sessions {
                tile.state = derive_visual_state(
                    &tile.session_id,
                    current_session_id,
                    current_session_running,
                );
            }
        }
        rows
    }

    fn import_initial_row(&mut self, current_session_id: Option<&str>, all_sessions: &[String]) {
        let sessions: Vec<String> = if all_sessions.is_empty() {
            current_session_id
                .map(|id| vec![id.to_string()])
                .unwrap_or_default()
        } else {
            all_sessions.to_vec()
        };

        if let Some(current) = current_session_id
            && !self.map.is_empty()
            && self.map.locate_session(current).is_some()
        {
            let _ = self.map.focus_session_by_id(current);
            return;
        }

        let focused_index = current_session_id
            .and_then(|current| sessions.iter().position(|session_id| session_id == current))
            .or_else(|| (!sessions.is_empty()).then_some(0));

        let tiles = sessions
            .into_iter()
            .map(WorkspaceSessionTile::new)
            .collect::<Vec<_>>();
        self.map.set_row_sessions(0, tiles, focused_index);
        self.map.set_current_workspace(0);
        self.imported_server_sessions = true;
    }
}

fn derive_visual_state(
    session_id: &str,
    current_session_id: Option<&str>,
    current_session_running: bool,
) -> WorkspaceSessionVisualState {
    if current_session_id == Some(session_id) {
        return if current_session_running {
            WorkspaceSessionVisualState::Running
        } else {
            WorkspaceSessionVisualState::Idle
        };
    }
    match Session::load(session_id).map(|session| session.status) {
        Ok(SessionStatus::Closed | SessionStatus::Reloaded | SessionStatus::Compacted) => {
            WorkspaceSessionVisualState::Completed
        }
        Ok(SessionStatus::RateLimited) => WorkspaceSessionVisualState::Waiting,
        Ok(SessionStatus::Error { .. } | SessionStatus::Crashed { .. }) => {
            WorkspaceSessionVisualState::Error
        }
        Ok(SessionStatus::Active) => WorkspaceSessionVisualState::Idle,
        Err(_) => WorkspaceSessionVisualState::Detached,
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceClientState, WorkspaceSplitTarget};

    #[test]
    fn enabling_imports_initial_sessions() {
        let mut state = WorkspaceClientState::default();
        state.enable(
            Some("session_a"),
            &["session_a".to_string(), "session_b".to_string()],
        );
        assert!(state.is_enabled());
        let rows = state.visible_rows(3, Some("session_a"), false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sessions.len(), 2);
        assert_eq!(rows[0].focused_index, Some(0));
    }

    #[test]
    fn horizontal_navigation_returns_new_target() {
        let mut state = WorkspaceClientState::default();
        state.enable(
            Some("session_a"),
            &["session_a".to_string(), "session_b".to_string()],
        );
        let next = state.navigate_right();
        assert_eq!(next.as_deref(), Some("session_b"));
    }

    #[test]
    fn split_response_in_workspace_targets_new_session() {
        let mut state = WorkspaceClientState::default();
        state.enable(Some("session_a"), &["session_a".to_string()]);
        state.queue_split_target(WorkspaceSplitTarget::Right);
        assert!(state.handle_split_response("session_child"));
        state.sync_after_history(
            "session_child",
            &["session_a".to_string(), "session_child".to_string()],
        );
        let rows = state.visible_rows(3, Some("session_child"), false);
        assert!(
            rows[0]
                .sessions
                .iter()
                .any(|tile| tile.session_id == "session_child")
        );
        assert_eq!(rows[0].focused_index, Some(1));
    }

    #[test]
    fn status_summary_reports_enabled_state() {
        let mut state = WorkspaceClientState::default();
        state.enable(Some("session_a"), &["session_a".to_string()]);
        let summary = state.status_summary();
        assert!(summary.contains("Workspace mode: on"));
    }
}
