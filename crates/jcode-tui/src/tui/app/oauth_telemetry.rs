use super::App;

use crate::telemetry::OAUTH_ACCOUNT_NOTICE_TEXT as NOTICE;

thread_local! {
    static NOTICE_AREA: std::cell::Cell<Option<ratatui::layout::Rect>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn begin_frame() {
    NOTICE_AREA.set(None);
}

pub(crate) fn note_status_render(area: ratatui::layout::Rect) {
    NOTICE_AREA.set(Some(area));
}

impl App {
    /// Only the credential bound to the selected provider is eligible. Never
    /// consult a global active-account fallback, especially for SSH clients.
    pub(super) fn poll_oauth_account_telemetry(&mut self) -> bool {
        if self.is_remote
            && self.remote_resolved_credential
                == Some(jcode_provider_core::ResolvedCredential::Oauth)
            && let (Some(provider), Some(email)) = (
                &self.remote_provider_name,
                &self.remote_provider_oauth_email,
            )
        {
            crate::telemetry::enqueue_oauth_account(provider, email.as_str());
        }
        if !self.is_remote
            && let Some(email) = self.provider.active_oauth_account_email()
        {
            let provider = self.provider.name();
            crate::telemetry::enqueue_oauth_account(provider, &email);
        }
        let expired = self
            .oauth_notice_displayed_at
            .is_some_and(|at| !readable_notice_active(at));
        if expired {
            self.oauth_notice_displayed_at = None;
        }
        expired
            || crate::telemetry::pending_oauth_account_notice().is_some()
            || self.oauth_notice_displayed_at.is_some()
    }

    /// Called only after the backend successfully flushed a completed frame.
    /// A clipped notice or an overlay hiding the status row is not an ack.
    pub(super) fn acknowledge_rendered_oauth_notice(&mut self, buffer: &ratatui::buffer::Buffer) {
        if NOTICE_AREA
            .get()
            .is_some_and(|area| notice_visible_in_area(buffer, area))
            && let Some(notice) = crate::telemetry::pending_oauth_account_notice()
        {
            if crate::telemetry::acknowledge_oauth_account_notice_displayed(&notice) {
                self.oauth_notice_displayed_at
                    .get_or_insert_with(std::time::Instant::now);
            }
        }
    }
}

pub(super) fn readable_notice_active(at: std::time::Instant) -> bool {
    at.elapsed() < std::time::Duration::from_secs(8)
}

fn notice_visible_in_area(buffer: &ratatui::buffer::Buffer, area: ratatui::layout::Rect) -> bool {
    let area = area.intersection(buffer.area);
    (area.y..area.bottom()).any(|y| {
        (area.x..area.right())
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
            .contains(NOTICE)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect, style::Style};

    #[test]
    fn oauth_notice_readable_interval_does_not_require_pending_upload() {
        let now = std::time::Instant::now();
        assert!(readable_notice_active(now));
        assert!(!readable_notice_active(
            now - std::time::Duration::from_secs(9)
        ));
    }

    #[test]
    fn oauth_notice_frame_marker_is_reset_for_hidden_status() {
        begin_frame();
        assert!(NOTICE_AREA.get().is_none());
        note_status_render(Rect::new(0, 1, 80, 1));
        assert_eq!(NOTICE_AREA.get(), Some(Rect::new(0, 1, 80, 1)));
        // A following overlay-only frame must not reuse a previous status row.
        begin_frame();
        assert!(NOTICE_AREA.get().is_none());
    }

    #[test]
    fn oauth_notice_ack_requires_complete_visible_text() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 2));
        assert!(!notice_visible_in_area(&buffer, buffer.area));
        buffer.set_string(0, 1, NOTICE, Style::default());
        assert!(notice_visible_in_area(&buffer, buffer.area));
        assert!(!notice_visible_in_area(&buffer, Rect::new(0, 0, 100, 1)));
        let mut clipped = Buffer::empty(Rect::new(0, 0, 30, 2));
        clipped.set_string(0, 1, NOTICE, Style::default());
        assert!(!notice_visible_in_area(&clipped, clipped.area));
    }
}
