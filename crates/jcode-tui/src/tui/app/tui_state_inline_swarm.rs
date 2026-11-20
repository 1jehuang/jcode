use super::*;

impl App {
    pub(super) fn inline_swarm_members_unticked(&self) -> Vec<crate::protocol::SwarmMemberStatus> {
        if self.debug_force_inline_gallery {
            return self.remote_swarm_members.clone();
        }
        if !self.swarm_enabled {
            return Vec::new();
        }
        // Scope the inline gallery to the subtree this session actually spawned.
        // Other sessions can share the same swarm (e.g. same repo) without this
        // session having spawned them; showing those would be noise. The spawn
        // tree is reconstructed from each member's `report_back_to_session_id`
        // parent edge.
        let self_id = if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        };
        match self_id {
            Some(self_id) => filter_inline_swarm_subtree(&self.remote_swarm_members, self_id),
            // Session identity is not known yet (e.g. right after connect,
            // before the History event sets `remote_session_id`). Showing all
            // swarm members here caused the inline strip to flash on startup
            // and then disappear once the subtree filter kicked in. Hide the
            // strip until we know who we are.
            None => Vec::new(),
        }
    }
}
