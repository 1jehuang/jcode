//! Keep transcript data and its loaded-state marker together across list refreshes.
use super::*;

fn same_transcript(a: &SessionInfo, b: &SessionInfo) -> bool {
    a.created_at == b.created_at
        && a.last_message_time == b.last_message_time
        && a.message_count == b.message_count
        && a.user_message_count == b.user_message_count
        && a.assistant_message_count == b.assistant_message_count
        && a.source == b.source
        && a.resume_target == b.resume_target
        && a.external_path == b.external_path
}

impl SessionPicker {
    /// List scans deliberately omit transcripts. Do not overwrite a loaded
    /// preview with that empty stub while retaining its extras/loaded sentinel.
    /// Changed transcripts instead lose the sentinel so selection reloads them.
    pub(super) fn reconcile_refreshed_previews(
        &mut self,
        groups: &mut [ServerGroup],
        orphans: &mut [SessionInfo],
    ) {
        let previous: HashMap<_, _> = self
            .all_sessions
            .iter()
            .chain(self.all_orphan_sessions.iter())
            .chain(
                self.all_server_groups
                    .iter()
                    .flat_map(|g| g.sessions.iter()),
            )
            .map(|session| (session.id.as_str(), session))
            .collect();
        let mut unchanged = HashSet::new();
        for incoming in orphans
            .iter_mut()
            .chain(groups.iter_mut().flat_map(|g| g.sessions.iter_mut()))
        {
            if let Some(old) = previous.get(incoming.id.as_str())
                && same_transcript(old, incoming)
            {
                if incoming.messages_preview.is_empty() {
                    incoming.messages_preview.clone_from(&old.messages_preview);
                }
                unchanged.insert(incoming.id.clone());
            }
        }
        self.preview_extras.retain(|id, _| unchanged.contains(id));
        // A fresh scan is also an opportunity to retry a previously unreadable
        // transcript, rather than keeping a permanent failure for this picker.
        self.preview_load_failures.clear();
        if self
            .pending_preview_load
            .as_ref()
            .is_some_and(|pending| !unchanged.contains(&pending.session_id))
        {
            // Dropping the receiver prevents an old in-flight read from being
            // applied to a newer transcript with the same session ID.
            self.pending_preview_load = None;
        }
    }
}

#[cfg(test)]
#[path = "preview_refresh_tests.rs"]
mod tests;
