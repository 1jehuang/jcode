//! Transport helpers for SSH session controls. IDs are opaque server values.
use super::*;

impl RemoteConnection {
    /// Switch the existing SSH connection, without consulting a local session file.
    /// Returning the request ID lets navigation roll back on a rejected resume.
    pub async fn resume_ssh_session(&mut self, session_id: &str) -> Result<u64> {
        let id = self.next_request_id;
        self.resume_session(session_id).await?;
        Ok(id)
    }

    /// A deliberate catch-up replay must not be discarded as bootstrap duplication.
    pub async fn refresh_ssh_history(&mut self) -> Result<u64> {
        let id = self.request_history().await?;
        self.has_loaded_history = false;
        Ok(id)
    }
}
