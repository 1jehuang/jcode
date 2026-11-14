use super::*;

/// How long queue dispatch waits for the server's by-id cancel answer before
/// falling back to requeueing every pending soft interrupt.
const SOFT_INTERRUPT_RECOVERY_TIMEOUT: Duration = Duration::from_secs(5);

impl App {
    pub(in crate::tui::app) fn track_pending_soft_interrupt(
        &mut self,
        request_id: u64,
        content: String,
    ) {
        let content_bytes = content.len();
        let content_chars = content.chars().count();
        if self.pending_soft_interrupts.is_empty() {
            self.sent_soft_interrupts.clear();
        }
        self.sent_soft_interrupts
            .push((request_id, content.clone()));
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
        self.sent_soft_interrupts.clear();
        self.soft_interrupt_recovery = None;
    }

    fn forget_sent_soft_interrupt(&mut self, content: &str) {
        if let Some(index) = self
            .sent_soft_interrupts
            .iter()
            .position(|(_, sent)| sent == content)
        {
            self.sent_soft_interrupts.remove(index);
        }
    }

    /// Server caller ids for every pending soft interrupt, or `None` when any
    /// of them cannot be named (restored from a reload snapshot, for example).
    fn pending_soft_interrupt_client_ids(&self) -> Option<Vec<String>> {
        let mut used = vec![false; self.sent_soft_interrupts.len()];
        let mut ids = Vec::with_capacity(self.pending_soft_interrupts.len());
        for pending in &self.pending_soft_interrupts {
            let index = self
                .sent_soft_interrupts
                .iter()
                .enumerate()
                .position(|(index, (_, sent))| !used[index] && sent == pending)?;
            used[index] = true;
            ids.push(crate::tui::backend::soft_interrupt_client_id(
                self.sent_soft_interrupts[index].0,
            ));
        }
        Some(ids)
    }

    /// True while the server has been asked which pending soft interrupts are
    /// still queued. Queue dispatch waits for the answer: requeueing first
    /// would resend a prompt the server already started as its own turn.
    pub(super) fn soft_interrupt_recovery_in_flight(&self) -> bool {
        self.soft_interrupt_recovery
            .is_some_and(|(_, started)| started.elapsed() < SOFT_INTERRUPT_RECOVERY_TIMEOUT)
    }

    /// Apply the server's verdict on a by-id recovery cancel. Only interrupts
    /// the server still had queued (or never acknowledged) are requeued. One it
    /// already accepted is running or injected, so requeueing would duplicate it.
    pub(super) fn finish_soft_interrupt_recovery(
        &mut self,
        id: u64,
        cancelled: &[String],
        not_queued: &[String],
    ) -> bool {
        if self.soft_interrupt_recovery.map(|(request, _)| request) != Some(id) {
            return false;
        }
        self.soft_interrupt_recovery = None;
        let mut recovered = Vec::new();
        let mut accepted = 0usize;
        for (client_id, was_queued) in cancelled
            .iter()
            .map(|id| (id, true))
            .chain(not_queued.iter().map(|id| (id, false)))
        {
            let Some(index) = self.sent_soft_interrupts.iter().position(|(request, _)| {
                &crate::tui::backend::soft_interrupt_client_id(*request) == client_id
            }) else {
                continue;
            };
            let (request_id, content) = self.sent_soft_interrupts.remove(index);
            let unacked = self
                .pending_soft_interrupt_requests
                .iter()
                .position(|(request, _)| *request == request_id)
                .map(|index| self.pending_soft_interrupt_requests.remove(index))
                .is_some();
            let Some(pending) = self
                .pending_soft_interrupts
                .iter()
                .position(|pending| pending == &content)
            else {
                continue;
            };
            self.pending_soft_interrupts.remove(pending);
            if was_queued || unacked {
                recovered.push(content);
            } else {
                accepted += 1;
            }
        }
        crate::logging::info(&format!(
            "REMOTE_SOFT_INTERRUPT_RECOVERY_RESULT id={} requeued={} already_accepted={}",
            id,
            recovered.len(),
            accepted
        ));
        if !recovered.is_empty() {
            recovered.append(&mut self.queued_messages);
            self.queued_messages = recovered;
            self.set_status_notice("Recovered queued interleave after turn finished");
        }
        true
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
            self.forget_sent_soft_interrupt(content);
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
                    self.forget_sent_soft_interrupt(&removed_content);
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

/// A busy rejection proves the send was not accepted, including user follow-ups
/// and automatic reminders. Disconnect recovery remains narrower because an
/// uncertain delivery could otherwise replay work the server already accepted.
pub(super) fn recover_rejected_queued_continuation(app: &mut App) -> bool {
    recover_queued_continuation(app, "server busy rejection", true)
}

fn recover_queued_continuation(app: &mut App, reason: &str, rejected: bool) -> bool {
    let is_recoverable = app
        .rate_limit_pending_message
        .as_ref()
        .is_some_and(|pending| {
            (rejected || pending.is_system && !pending.auto_retry)
                && (!pending.content.trim().is_empty()
                    || !pending.images.is_empty()
                    || pending.system_reminder.is_some())
        });
    if !is_recoverable {
        return false;
    }
    let Some(pending) = app.rate_limit_pending_message.take() else {
        return false;
    };
    app.rate_limit_reset = None;
    crate::logging::info(&format!(
        "Recovering rejected in-flight follow-up into queued messages after {} (content_chars={}, has_reminder={})",
        reason,
        pending.content.chars().count(),
        pending.system_reminder.is_some()
    ));
    if let Some(reminder) = pending.system_reminder {
        app.hidden_queued_system_messages.insert(0, reminder);
    }
    if !pending.content.trim().is_empty() || !pending.images.is_empty() {
        app.queued_messages.insert(0, pending.content);
        app.queued_message_images.splice(0..0, pending.images);
    }
    true
}

pub(super) fn recover_local_interleave_to_queue(app: &mut App, reason: &str) -> bool {
    let Some(interleave) = app.interleave_message.take() else {
        return false;
    };
    if interleave.trim().is_empty() && app.interleave_images.is_empty() {
        app.interleave_images.clear();
        return false;
    }

    crate::logging::info(&format!(
        "Recovering unsent interleave into queued follow-ups after {}",
        reason
    ));
    app.queued_messages.insert(0, interleave);
    app.queued_message_images
        .splice(0..0, std::mem::take(&mut app.interleave_images));
    true
}

pub(super) async fn recover_stranded_soft_interrupts(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> bool {
    if app.pending_soft_interrupts.is_empty() {
        app.soft_interrupt_recovery = None;
        return false;
    }
    if app.is_processing || app.soft_interrupt_recovery_in_flight() {
        return false;
    }
    // The turn ended on this client, but the server may already have accepted
    // a pending interrupt: one that reaches an idle session starts its own
    // turn. Ask which are still queued and requeue only those. Blind requeueing
    // sent the same prompt twice and drew "Already processing a message".
    let unanswered = app.soft_interrupt_recovery.take().is_some();
    if !unanswered && let Some(client_ids) = app.pending_soft_interrupt_client_ids() {
        match remote.cancel_soft_interrupts_by_id(client_ids).await {
            Ok(id) => app.soft_interrupt_recovery = Some((id, Instant::now())),
            Err(err) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to recover queued interleave message: {}",
                    err
                )));
                app.set_status_notice("Queued interleave recovery failed");
            }
        }
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
    app.sent_soft_interrupts.clear();

    let mut recovered_queue = recovered_interrupts;
    recovered_queue.append(&mut app.queued_messages);
    app.queued_messages = recovered_queue;
    app.set_status_notice("Recovered queued interleave after turn finished");
    true
}
