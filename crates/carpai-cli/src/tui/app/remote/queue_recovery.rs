use super::*;

impl App {
    pub(super) fn track_pending_soft_interrupt(&mut self, request_id: u64, content: String) {
        self.pending_soft_interrupt_requests
            .push((request_id, content.clone()));
        self.pending_soft_interrupts.push(content);
    }

    pub(super) fn acknowledge_pending_soft_interrupt(&mut self, request_id: u64) -> bool {
        if let Some(index) = self
            .pending_soft_interrupt_requests
            .iter()
            .position(|(id, _)| *id == request_id)
        {
            self.pending_soft_interrupt_requests.remove(index);
            true
        } else {
            false
        }
    }

    pub(super) fn clear_pending_soft_interrupt_tracking(&mut self) {
        self.pending_soft_interrupts.clear();
        self.pending_soft_interrupt_requests.clear();
    }

    pub(super) fn mark_soft_interrupt_injected(&mut self, content: &str) {
        if self.mark_combined_soft_interrupt_injected(content) {
            return;
        }

        if let Some(index) = self
            .pending_soft_interrupts
            .iter()
            .position(|pending| pending == content)
        {
            self.pending_soft_interrupts.remove(index);
        }

        if let Some(index) = self
            .pending_soft_interrupt_requests
            .iter()
            .position(|(_, pending)| pending == content)
        {
            self.pending_soft_interrupt_requests.remove(index);
        }
    }

    fn mark_combined_soft_interrupt_injected(&mut self, content: &str) -> bool {
        let mut combined = String::new();
        for (index, pending) in self.pending_soft_interrupts.iter().enumerate() {
            if index > 0 {
                combined.push_str("\n\n");
            }
            combined.push_str(pending);

            if combined == content {
                let count = index + 1;
                let removed: Vec<String> = self.pending_soft_interrupts.drain(..count).collect();
                for removed_content in removed {
                    if let Some(request_index) = self
                        .pending_soft_interrupt_requests
                        .iter()
                        .position(|(_, pending)| pending == &removed_content)
                    {
                        self.pending_soft_interrupt_requests.remove(request_index);
                    }
                }
                return true;
            }

            if !content.starts_with(&combined) {
                break;
            }
        }

        false
    }
}

pub(super) fn recover_local_interleave_to_queue(app: &mut App, reason: &str) -> bool {
    let Some(interleave) = app.interleave_message.take() else {
        return false;
    };
    if interleave.trim().is_empty() {
        return false;
    }

    crate::logging::info(&format!(
        "Recovering unsent interleave into queued follow-ups after {}",
        reason
    ));
    app.queued_messages.insert(0, interleave);
    true
}

pub(super) async fn recover_stranded_soft_interrupts(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> bool {
    if app.is_processing || app.pending_soft_interrupts.is_empty() {
        return false;
    }

    let recovered_interrupts = std::mem::take(&mut app.pending_soft_interrupts);
    if recovered_interrupts.is_empty() {
        return false;
    }

    if let Err(err) = remote.cancel_soft_interrupts().await {
        app.pending_soft_interrupts = recovered_interrupts;
        app.push_display_message(DisplayMessage::error(format!(
            "Failed to recover queued interleave message: {}",
            err
        )));
        app.set_status_notice("Queued interleave recovery failed");
        return false;
    }

    crate::logging::info(&format!(
        "Recovering {} stranded soft interrupt(s) into queued follow-ups after turn boundary",
        recovered_interrupts.len()
    ));
    app.pending_soft_interrupt_requests.clear();

    let mut recovered_queue = recovered_interrupts;
    recovered_queue.append(&mut app.queued_messages);
    app.queued_messages = recovered_queue;
    app.set_status_notice("Recovered queued interleave after turn finished");
    true
}
