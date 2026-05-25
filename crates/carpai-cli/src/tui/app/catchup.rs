use super::{App, PendingCatchupResume};
use crate::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};

pub(super) const CATCHUP_PAGE_ID: &str = "catchup";
const CATCHUP_PAGE_TITLE: &str = "Catch Up";

impl App {
    pub(super) fn queue_catchup_resume(
        &mut self,
        target_session_id: String,
        source_session_id: Option<String>,
        queue_position: Option<(usize, usize)>,
        show_brief: bool,
    ) {
        self.pending_catchup_resume = Some(PendingCatchupResume {
            target_session_id,
            source_session_id,
            queue_position,
            show_brief,
        });
    }

    pub(super) fn take_pending_catchup_resume(&mut self) -> Option<PendingCatchupResume> {
        self.pending_catchup_resume.take()
    }

    pub(super) fn begin_in_flight_catchup_resume(&mut self, request: PendingCatchupResume) {
        if request.show_brief
            && let Some(source) = request.source_session_id.as_ref()
            && self.catchup_return_stack.last() != Some(source)
        {
            self.catchup_return_stack.push(source.clone());
        }
        self.in_flight_catchup_resume = Some(request);
    }

    pub(super) fn clear_in_flight_catchup_resume(&mut self) {
        self.in_flight_catchup_resume = None;
    }

    pub(super) fn maybe_show_catchup_after_history(&mut self, session_id: &str) {
        let Some(request) = self.in_flight_catchup_resume.clone() else {
            return;
        };
        if request.target_session_id != session_id {
            return;
        }
        self.in_flight_catchup_resume = None;
        if !request.show_brief {
            return;
        }

        let Ok(session) = crate::session::Session::load(session_id) else {
            self.push_display_message(crate::tui::DisplayMessage::error(format!(
                "Catch Up loaded session `{}` but could not read its persisted state.",
                session_id
            )));
            return;
        };

        let brief = crate::catchup::build_brief(&session);
        let markdown = crate::catchup::render_markdown(
            &session,
            request.source_session_id.as_deref(),
            request.queue_position,
            &brief,
        );
        let mut snapshot = self.snapshot_without_catchup();
        snapshot.pages.push(self.catchup_page(session_id, markdown));
        snapshot.pages.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        snapshot.focused_page_id = Some(CATCHUP_PAGE_ID.to_string());
        self.apply_side_panel_snapshot(snapshot);
        let _ = crate::catchup::mark_seen(&session.id, session.updated_at);
    }

    pub(super) fn snapshot_without_catchup(&self) -> SidePanelSnapshot {
        let mut snapshot = self.side_panel.clone();
        snapshot.pages.retain(|page| page.id != CATCHUP_PAGE_ID);
        if snapshot.focused_page_id.as_deref() == Some(CATCHUP_PAGE_ID) {
            snapshot.focused_page_id = None;
        }
        snapshot
    }

    pub(super) fn pop_catchup_return_target(&mut self) -> Option<String> {
        self.catchup_return_stack.pop()
    }

    fn catchup_page(&self, session_id: &str, markdown: String) -> SidePanelPage {
        SidePanelPage {
            id: CATCHUP_PAGE_ID.to_string(),
            title: CATCHUP_PAGE_TITLE.to_string(),
            file_path: format!("catchup://{}", session_id),
            format: SidePanelPageFormat::Markdown,
            source: SidePanelPageSource::Ephemeral,
            content: markdown,
            updated_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(1)
                .max(1),
        }
    }
}
