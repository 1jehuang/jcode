use super::*;

/// Caller id attached to a TUI soft interrupt, derived from its request id.
pub(crate) fn soft_interrupt_client_id(request_id: u64) -> String {
    format!("tui-{request_id}")
}

impl RemoteConnection {
    /// Queue a soft interrupt message to be injected at the next safe point
    /// This doesn't cancel anything - the message is naturally incorporated
    pub async fn soft_interrupt(
        &mut self,
        content: String,
        images: Vec<(String, String)>,
        urgent: bool,
    ) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::SoftInterrupt {
            id,
            content,
            images,
            urgent,
            // Named so stranded-interrupt recovery can ask the server exactly
            // which interrupts are still queued instead of guessing.
            client_id: Some(soft_interrupt_client_id(id)),
        };
        self.next_request_id += 1;
        self.send_request_with_interrupt_trigger(request, Some("soft_interrupt"))
            .await?;
        Ok(id)
    }

    /// Cancel only the named soft interrupts. The server answers with
    /// `SoftInterruptsCancelled`, splitting the ids into still-queued (now
    /// removed) and not-queued (already injected or started as a turn).
    pub async fn cancel_soft_interrupts_by_id(&mut self, client_ids: Vec<String>) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::CancelSoftInterrupts {
            id,
            client_ids: Some(client_ids),
        };
        self.next_request_id += 1;
        self.send_request_with_interrupt_trigger(request, Some("cancel_soft_interrupts"))
            .await?;
        Ok(id)
    }

    pub async fn cancel_soft_interrupts(&mut self) -> Result<()> {
        let request = Request::CancelSoftInterrupts {
            id: self.next_request_id,
            client_ids: None,
        };
        self.next_request_id += 1;
        self.send_request_with_interrupt_trigger(request, Some("cancel_soft_interrupts"))
            .await
    }
}
