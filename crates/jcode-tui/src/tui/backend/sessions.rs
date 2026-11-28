use super::*;

impl RemoteConnection {
    /// Resume a specific session by ID
    pub async fn resume_session(&mut self, session_id: &str) -> Result<u64> {
        if self.session_id.as_deref() != Some(session_id) {
            let had_cloud_authority = self.cloud_authority.is_some()
                || self.jarvis_tools_configured
                || self.local_tools_request.is_some();
            self.revoke_cloud_authority();
            if had_cloud_authority {
                self.clear_local_tools().await?;
            }
        }
        let id = self.next_request_id;
        let request = Request::ResumeSession {
            id,
            session_id: session_id.to_string(),
            client_instance_id: self.client_instance_id.clone(),
            client_has_local_history: false,
            allow_session_takeover: false,
            client_name: None,
        };
        self.next_request_id += 1;
        self.send_request(request).await?;
        // An explicit picker/workspace switch must accept the target's History
        // even when a SessionId event (or reload-time local restore) associated
        // this connection with the target before History arrived. Otherwise
        // `has_loaded_history` still describes the source session and the TUI
        // drops the target replay as a duplicate, leaving an orphan resume with
        // an empty transcript despite its intact persisted messages (#753).
        // History application replaces the display vector, so repeated payloads
        // remain idempotent rather than appending duplicate messages.
        self.has_loaded_history = false;
        Ok(id)
    }
}
