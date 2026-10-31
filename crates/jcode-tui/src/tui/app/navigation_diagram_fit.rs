use super::*;

impl App {
    pub(super) fn current_visible_diagram_hash(&self) -> Option<u64> {
        if self.diagram_mode != crate::config::DiagramDisplayMode::Pinned
            || !self.diagram_pane_enabled
        {
            return None;
        }
        if self.side_panel.focused_page().is_some()
            && self.diagram_pane_position == crate::config::DiagramPanePosition::Side
        {
            return None;
        }
        let diagrams = crate::tui::mermaid::get_active_diagrams();
        diagrams
            .get(self.diagram_index.min(diagrams.len().saturating_sub(1)))
            .map(|diagram| diagram.hash)
    }

    pub(in crate::tui::app) fn reset_diagram_view_to_fit(&mut self) {
        self.diagram_scroll_x = 0;
        self.diagram_scroll_y = 0;
        self.diagram_zoom = 100;
    }

    pub(in crate::tui::app) fn sync_diagram_fit_context(&mut self) {
        let current_hash = self.current_visible_diagram_hash();
        if current_hash != self.last_visible_diagram_hash {
            self.reset_diagram_view_to_fit();
            self.last_visible_diagram_hash = current_hash;
        }
    }

    pub(in crate::tui::app) fn handle_diagram_geometry_change(&mut self) {
        self.reset_diagram_view_to_fit();
        if self.side_panel.focused_page().is_some() {
            self.diff_pane_scroll_x = 0;
        }
        crate::tui::mermaid::clear_image_state();
        crate::tui::clear_side_panel_render_caches();
        self.last_visible_diagram_hash = self.current_visible_diagram_hash();
    }
}
