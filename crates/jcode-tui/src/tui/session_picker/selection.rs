//! Keep explicit session choices intact across asynchronous list refreshes.

use super::*;

impl SessionPicker {
    pub(super) fn toggle_selected_session(&mut self) {
        let Some(session_id) = self.selected_session().map(|session| session.id.clone()) else {
            return;
        };
        self.user_selection_intent = true;
        if !self.selected_session_ids.insert(session_id.clone()) {
            self.selected_session_ids.remove(&session_id);
        }
    }

    pub(super) fn apply_session_list_update(&mut self, update: SessionListUpdate) -> Result<bool> {
        let (sessions, finished) = match update {
            SessionListUpdate::Cached(sessions) => (sessions, false),
            SessionListUpdate::Fresh(sessions) => (sessions?, true),
        };
        // An untouched cached highlight must not suppress the project default.
        // Typed searches and explicit navigation keep their target and filter.
        let apply_project_default = !self.user_selection_intent && self.search_query.is_empty();
        self.reseed_grouped(sessions.0, sessions.1);
        if apply_project_default {
            self.apply_resume_project_scope_default();
        }
        Ok(finished)
    }
}
