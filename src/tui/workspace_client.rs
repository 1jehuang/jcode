use crate::session::{Session, SessionStatus};
use crate::tui::workspace_map::{
    VisibleWorkspaceRow, WorkspaceMapModel, WorkspaceSessionTile, WorkspaceSessionVisualState,
};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSplitTarget {
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Default)]
struct WorkspaceClientState {
    enabled: bool,
    map: WorkspaceMapModel,
    imported_server_sessions: bool,
    pending_split_target: Option<WorkspaceSplitTarget>,
    pending_resume_session: Option<String>,
}

static WORKSPACE_STATE: Mutex<Option<WorkspaceClientState>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut WorkspaceClientState) -> R) -> R {
    let mut guard = WORKSPACE_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(WorkspaceClientState::default);
    f(state)
}

pub fn is_enabled() -> bool {
    with_state(|state| state.enabled)
}

pub fn enable(current_session_id: Option<&str>, all_sessions: &[String]) {
    with_state(|state| {
        state.enabled = true;
        if state.map.is_empty() {
            import_initial_row(state, current_session_id, all_sessions);
        } else if let Some(session_id) = current_session_id {
            let _ = state.map.focus_session_by_id(session_id);
        }
    });
}

pub fn disable() {
    with_state(|state| {
        state.enabled = false;
        state.pending_split_target = None;
        state.pending_resume_session = None;
    });
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let mut guard = WORKSPACE_STATE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

pub fn status_summary() -> String {
    with_state(|state| {
        if !state.enabled {
            return "Workspace mode: off".to_string();
        }
        let rows = state.map.visible_rows(5);
        let populated = state.map.populated_workspaces().len();
        let total_sessions: usize = rows.iter().map(|row| row.sessions.len()).sum();
        format!(
            "Workspace mode: on\nCurrent workspace: {}\nVisible rows: {}\nPopulated workspaces: {}\nMapped sessions: {}",
            state.map.current_workspace(),
            rows.len(),
            populated,
            total_sessions
        )
    })
}

pub fn sync_after_history(current_session_id: &str, all_sessions: &[String]) {
    with_state(|state| {
        if !state.enabled {
            return;
        }
        if state.map.is_empty() {
            import_initial_row(state, Some(current_session_id), all_sessions);
            return;
        }
        if state.map.focus_session_by_id(current_session_id) {
            return;
        }
        let tile = WorkspaceSessionTile::new(current_session_id.to_string());
        let _ = state.map.add_session_to_current_workspace(tile);
    });
}

pub fn queue_split_target(target: WorkspaceSplitTarget) {
    with_state(|state| {
        state.enabled = true;
        state.pending_split_target = Some(target);
    });
}

pub fn take_pending_resume_session() -> Option<String> {
    with_state(|state| state.pending_resume_session.take())
}

pub fn handle_split_response(new_session_id: &str) -> bool {
    with_state(|state| {
        if !state.enabled || state.pending_split_target.is_none() {
            state.pending_split_target = None;
            return false;
        }
        let target = state
            .pending_split_target
            .take()
            .unwrap_or(WorkspaceSplitTarget::Right);
        let target_workspace = match target {
            WorkspaceSplitTarget::Right => state.map.current_workspace(),
            WorkspaceSplitTarget::Up => state.map.current_workspace() + 1,
            WorkspaceSplitTarget::Down => state.map.current_workspace() - 1,
        };
        let _ = state.map.insert_session_in_workspace(
            target_workspace,
            WorkspaceSessionTile::new(new_session_id.to_string()),
        );
        let _ = state.map.focus_session_by_id(new_session_id);
        state.pending_resume_session = Some(new_session_id.to_string());
        true
    })
}

pub fn navigate_left() -> Option<String> {
    with_state(|state| {
        if !state.enabled || !state.map.move_left() {
            return None;
        }
        state
            .map
            .current_focused_session_id()
            .map(ToString::to_string)
    })
}

pub fn navigate_right() -> Option<String> {
    with_state(|state| {
        if !state.enabled || !state.map.move_right() {
            return None;
        }
        state
            .map
            .current_focused_session_id()
            .map(ToString::to_string)
    })
}

pub fn navigate_up() -> Option<String> {
    with_state(|state| {
        if !state.enabled {
            return None;
        }
        let target_workspace = state.map.nearest_populated_workspace_above()?;
        state.map.set_current_workspace(target_workspace);
        state
            .map
            .focused_session_in_workspace(target_workspace)
            .map(ToString::to_string)
    })
}

pub fn navigate_down() -> Option<String> {
    with_state(|state| {
        if !state.enabled {
            return None;
        }
        let target_workspace = state.map.nearest_populated_workspace_below()?;
        state.map.set_current_workspace(target_workspace);
        state
            .map
            .focused_session_in_workspace(target_workspace)
            .map(ToString::to_string)
    })
}

pub fn visible_rows(
    max_rows: usize,
    current_session_id: Option<&str>,
    current_session_running: bool,
) -> Vec<VisibleWorkspaceRow> {
    with_state(|state| {
        let mut rows = if state.enabled {
            state.map.visible_rows(max_rows)
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
    })
}

fn import_initial_row(
    state: &mut WorkspaceClientState,
    current_session_id: Option<&str>,
    all_sessions: &[String],
) {
    let sessions: Vec<String> = if all_sessions.is_empty() {
        current_session_id
            .map(|id| vec![id.to_string()])
            .unwrap_or_default()
    } else {
        all_sessions.to_vec()
    };

    if let Some(current) = current_session_id {
        if !state.map.is_empty() && state.map.locate_session(current).is_some() {
            let _ = state.map.focus_session_by_id(current);
            return;
        }
    }

    let focused_index = current_session_id
        .and_then(|current| sessions.iter().position(|session_id| session_id == current))
        .or_else(|| (!sessions.is_empty()).then_some(0));

    let tiles = sessions
        .into_iter()
        .map(WorkspaceSessionTile::new)
        .collect::<Vec<_>>();
    state.map.set_row_sessions(0, tiles, focused_index);
    state.map.set_current_workspace(0);
    state.imported_server_sessions = true;
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
    use super::{
        WorkspaceSplitTarget, enable, handle_split_response, is_enabled, navigate_right,
        queue_split_target, reset_for_tests, status_summary, sync_after_history, visible_rows,
    };
    use std::sync::{Mutex, OnceLock};

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("workspace test lock")
    }

    fn reset() {
        reset_for_tests();
    }

    #[test]
    fn enabling_imports_initial_sessions() {
        let _guard = test_lock();
        reset();
        enable(
            Some("session_a"),
            &["session_a".to_string(), "session_b".to_string()],
        );
        assert!(is_enabled());
        let rows = visible_rows(3, Some("session_a"), false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sessions.len(), 2);
        assert_eq!(rows[0].focused_index, Some(0));
    }

    #[test]
    fn horizontal_navigation_returns_new_target() {
        let _guard = test_lock();
        reset();
        enable(
            Some("session_a"),
            &["session_a".to_string(), "session_b".to_string()],
        );
        let next = navigate_right();
        assert_eq!(next.as_deref(), Some("session_b"));
    }

    #[test]
    fn split_response_in_workspace_targets_new_session() {
        let _guard = test_lock();
        reset();
        enable(Some("session_a"), &["session_a".to_string()]);
        queue_split_target(WorkspaceSplitTarget::Right);
        assert!(handle_split_response("session_child"));
        sync_after_history(
            "session_child",
            &["session_a".to_string(), "session_child".to_string()],
        );
        let rows = visible_rows(3, Some("session_child"), false);
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
        let _guard = test_lock();
        reset();
        enable(Some("session_a"), &["session_a".to_string()]);
        let summary = status_summary();
        assert!(summary.contains("Workspace mode: on"));
    }
}
