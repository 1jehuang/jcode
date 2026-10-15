//! Side-panel visibility controls, preserving retained pages when dismissed.
use super::*;

impl App {
    pub(in crate::tui::app) fn dismiss_side_panel(&mut self) {
        self.side_panel_fullscreen = false;
        crate::tui::clear_side_panel_render_caches();
        self.last_side_panel_focus_id = self.side_panel.focused_page_id.clone();
        self.side_panel.focused_page_id = None;
        self.side_panel_user_hidden = true;
        self.side_panel_explicit_hidden = true;
        self.pinned_images_auto_hide_deadline = None;
        if !self.diff_pane_visible() {
            self.set_diff_pane_focus(false);
        }
        self.sync_diagram_fit_context();
        self.set_status_notice("Side panel: OFF");
    }

    pub(in crate::tui::app) fn toggle_side_panel(&mut self) {
        if self.side_panel_user_hidden {
            self.side_panel_user_hidden = false;
            self.side_panel_explicit_hidden = false;
            self.pinned_images_auto_hide_deadline = None;
            if self.side_panel.pages.is_empty() {
                if self.side_pane_has_visual_images_ignoring_user_hidden() {
                    self.sync_diagram_fit_context();
                    self.set_status_notice("Image side panel: ON");
                } else {
                    self.notify_no_side_panel_pages();
                }
                return;
            }
        }

        if self.side_pane_has_visual_images() {
            self.side_panel_user_hidden = true;
            self.side_panel_explicit_hidden = true;
            self.pinned_images_auto_hide_deadline = None;
            self.set_diff_pane_focus(false);
            self.sync_diagram_fit_context();
            self.set_status_notice("Image side panel: OFF");
            return;
        }

        if self.side_panel.pages.is_empty() {
            self.notify_no_side_panel_pages();
            return;
        }

        if let Some(page) = self.side_panel.focused_page() {
            let title = page.title.clone();
            // Alt+M cycle: split -> fullscreen -> hidden -> split.
            if !self.side_panel_fullscreen {
                self.side_panel_fullscreen = true;
                self.sync_diagram_fit_context();
                crate::tui::clear_side_panel_render_caches();
                self.set_status_notice(format!("Side panel: {title} (fullscreen)"));
                return;
            }
            self.dismiss_side_panel();
            return;
        }

        let restore_id = self
            .last_side_panel_focus_id
            .as_deref()
            .filter(|id| self.side_panel.pages.iter().any(|page| page.id == *id))
            .map(str::to_owned)
            .or_else(|| self.side_panel.pages.first().map(|page| page.id.clone()));

        let Some(restore_id) = restore_id else {
            self.notify_no_side_panel_pages();
            return;
        };

        self.side_panel.focused_page_id = Some(restore_id.clone());
        self.last_side_panel_focus_id = Some(restore_id);
        self.side_panel_fullscreen = false;
        self.side_panel_user_hidden = false;
        self.side_panel_explicit_hidden = false;
        self.sync_diagram_fit_context();
        let status = self
            .side_panel
            .focused_page()
            .map(|page| format!("Side panel: {}", page.title))
            .unwrap_or_else(|| "Side panel: ON".to_string());
        self.set_status_notice(status);
    }

    fn notify_no_side_panel_pages(&mut self) {
        let diagram_key = crate::tui::keybind::diagram_pane_visibility_key_label();
        self.set_status_notice(format!(
            "Side panel: no pages ({diagram_key} toggles diagrams)"
        ));
    }
}
