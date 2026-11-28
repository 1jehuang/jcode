use super::*;

impl RemoteConnection {
    /// Set reasoning effort on the server (for OpenAI models)
    pub async fn set_reasoning_effort(&mut self, effort: &str) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::SetReasoningEffort {
            id,
            effort: effort.to_string(),
            target_session_id: None,
        };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }
}
