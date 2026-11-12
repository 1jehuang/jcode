use super::*;

impl App {
    pub(super) fn track_pending_soft_interrupt(&mut self, request_id: u64, content: String) {
        let content_bytes = content.len();
        let content_chars = content.chars().count();
        self.pending_soft_interrupt_requests
            .push((request_id, content.clone()));
        self.pending_soft_interrupts.push(content);
        crate::logging::info(&format!(
            "REMOTE_SOFT_INTERRUPT_TRACK_PENDING id={} content_bytes={} content_chars={} pending_requests={} pending_messages={}",
            request_id,
            content_bytes,
            content_chars,
            self.pending_soft_interrupt_requests.len(),
            self.pending_soft_interrupts.len()
        ));
    }

    pub(super) fn acknowledge_pending_soft_interrupt(&mut self, request_id: u64) -> bool {
        if let Some(index) = self
            .pending_soft_interrupt_requests
            .iter()
            .position(|(id, _)| *id == request_id)
        {
            self.pending_soft_interrupt_requests.remove(index);
            crate::logging::info(&format!(
                "REMOTE_SOFT_INTERRUPT_ACK_MATCHED id={} pending_requests={} pending_messages={}",
                request_id,
                self.pending_soft_interrupt_requests.len(),
                self.pending_soft_interrupts.len()
            ));
            true
        } else {
            if !self.pending_soft_interrupt_requests.is_empty() {
                crate::logging::info(&format!(
                    "REMOTE_SOFT_INTERRUPT_ACK_UNMATCHED id={} pending_requests={} pending_messages={}",
                    request_id,
                    self.pending_soft_interrupt_requests.len(),
                    self.pending_soft_interrupts.len()
                ));
            }
            false
        }
    }

    pub(super) fn clear_pending_soft_interrupt_tracking(&mut self) {
        crate::logging::info(&format!(
            "REMOTE_SOFT_INTERRUPT_TRACKING_CLEAR pending_requests={} pending_messages={}",
            self.pending_soft_interrupt_requests.len(),
            self.pending_soft_interrupts.len()
        ));
        self.pending_soft_interrupts.clear();
        self.pending_soft_interrupt_requests.clear();
    }

    pub(super) fn mark_soft_interrupt_injected(&mut self, content: &str) {
        crate::logging::info(&format!(
            "REMOTE_SOFT_INTERRUPT_MARK_INJECTED content_bytes={} content_chars={} pending_requests={} pending_messages={}",
            content.len(),
            content.chars().count(),
            self.pending_soft_interrupt_requests.len(),
            self.pending_soft_interrupts.len()
        ));
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

/// Recover an in-flight queued continuation back into the queue.
///
/// A queued follow-up that was already taken from `queued_messages` and handed
/// to `begin_remote_send` lives only in `rate_limit_pending_message` while it
/// is in flight. That pending shape (`is_system` with `auto_retry == false`)
/// has no retry path: the tick resend requires a rate-limit reset timestamp
/// and the disconnect resend requires `auto_retry`. If the connection dies
/// before the turn completes (typically a server reload handoff racing the
/// dispatch), clearing the pending message silently drops the user's queued
/// message (issue #391). Instead, put it back at the front of the queue so it
/// is re-sent once the turn is proven idle after reconnect, which is the
/// queue's contract.
pub(super) fn recover_undelivered_queued_continuation(app: &mut App, reason: &str) -> bool {
    recover_queued_continuation(app, reason, false)
}

/// A busy rejection proves the send was not accepted, including automatic
/// reminders. Unlike a disconnect, it is safe to queue these without risking
/// replaying a continuation that the server is already running.
pub(super) fn recover_rejected_queued_continuation(app: &mut App) -> bool {
    recover_queued_continuation(app, "server busy rejection", true)
}

fn recover_queued_continuation(app: &mut App, reason: &str, rejected: bool) -> bool {
    let is_recoverable = app
        .rate_limit_pending_message
        .as_ref()
        .is_some_and(|pending| {
            pending.is_system
                && (rejected || !pending.auto_retry)
                && (!pending.content.trim().is_empty() || pending.system_reminder.is_some())
        });
    if !is_recoverable {
        return false;
    }
    let Some(pending) = app.rate_limit_pending_message.take() else {
        return false;
    };
    app.rate_limit_reset = None;
    crate::logging::info(&format!(
        "Recovering in-flight queued continuation into queued follow-ups after {} (content_chars={}, has_reminder={})",
        reason,
        pending.content.chars().count(),
        pending.system_reminder.is_some()
    ));
    if let Some(reminder) = pending.system_reminder {
        app.hidden_queued_system_messages.insert(0, reminder);
    }
    if !pending.content.trim().is_empty() {
        app.queued_messages.insert(0, pending.content);
    }
    true
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
