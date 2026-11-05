use super::*;

impl CopyBadgeUiState {
    fn pulse_active(expires_at: Option<Instant>, now: Instant) -> bool {
        expires_at.is_some_and(|expires_at| expires_at > now)
    }

    pub(crate) fn alt_is_active(&self, now: Instant) -> bool {
        self.alt_active || Self::pulse_active(self.alt_pulse_until, now)
    }

    pub(crate) fn shift_is_active(&self, now: Instant) -> bool {
        self.alt_is_active(now)
            && (self.shift_active || Self::pulse_active(self.shift_pulse_until, now))
    }

    pub(crate) fn key_is_active(&self, key: char, now: Instant) -> bool {
        self.shift_is_active(now)
            && self
                .key_active
                .as_ref()
                .map(|(active_key, expires_at)| {
                    active_key.eq_ignore_ascii_case(&key) && *expires_at > now
                })
                .unwrap_or(false)
    }

    pub(crate) fn feedback_for_key(&self, key: char, now: Instant) -> Option<bool> {
        self.copied_feedback.as_ref().and_then(|feedback| {
            if feedback.key.eq_ignore_ascii_case(&key) && feedback.expires_at > now {
                Some(feedback.success)
            } else {
                None
            }
        })
    }

    pub(crate) fn expand_feedback_is_active(&self, now: Instant) -> bool {
        self.expand_feedback_until
            .is_some_and(|expires_at| expires_at > now)
    }
}
