use super::{RemoteConnection, Request, Result};

impl RemoteConnection {
    /// Cycle the active model on the server
    pub async fn cycle_model(&mut self, direction: i8) -> Result<()> {
        let request = Request::CycleModel {
            id: self.next_request_id,
            direction,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Trigger a background refresh of available models on the server.
    pub async fn refresh_models(&mut self) -> Result<()> {
        let request = Request::RefreshModels {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Focus or reopen a retained panel on the attached daemon, including SSH.
    pub async fn panel_action(
        &mut self,
        session_id: &str,
        action: &str,
        page_id: &str,
    ) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::PanelAction {
            id,
            session_id: session_id.to_string(),
            action: action.to_string(),
            page_id: page_id.to_string(),
        })
        .await?;
        Ok(id)
    }
}
