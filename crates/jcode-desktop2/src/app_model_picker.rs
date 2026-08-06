//! Pointer and harness actions for the model menu below the composer.

use crate::{App, harness};

impl App {
    pub(crate) fn has_model_caption(&self) -> bool {
        self.model
            .model
            .as_ref()
            .and_then(|id| id.caption())
            .is_some()
    }

    /// Toggle the catalog from either the caption or its advertised Ctrl+M
    /// chord. Keeping this in one path makes pointer and keyboard behavior
    /// identical, including connection errors and the single-menu rule.
    pub(crate) fn toggle_model_picker(&mut self) {
        if self.model.model_picker.is_open() {
            self.model.model_picker.close();
            self.request_redraw();
            return;
        }
        if !self.has_model_caption() {
            return;
        }
        let Some((_, outgoing)) = self.harness.as_ref() else {
            self.model.set_notice("not connected: cannot list models");
            self.request_redraw();
            return;
        };
        if outgoing.send(harness::Command::ListModels).is_err() {
            self.model.set_notice("not connected: cannot list models");
            self.request_redraw();
            return;
        }
        self.model.panel.close();
        self.model.model_picker.open_loading();
        self.request_redraw();
    }

    /// Let the caption or its open menu consume a press before the composer and
    /// transcript see it. Like the settings panel, dismiss clicks are consumed
    /// so closing a menu cannot also move the caret underneath it.
    pub(crate) fn model_picker_press(&mut self, x: f64, y: f64) -> bool {
        let on_button = self.has_model_caption() && self.frame.hits_model_button(x, y);
        if self.model.model_picker.is_open() {
            if on_button {
                self.toggle_model_picker();
                return true;
            }
            let rows = self.model.model_picker.visual_rows();
            if let Some(index) = self.frame.model_menu_row_at(rows, x, y) {
                if let Some(model) = self.model.model_picker.choose_row(index) {
                    self.model.model_picker.close();
                    if let Some((_, outgoing)) = self.harness.as_ref() {
                        if outgoing.send(harness::Command::SetModel(model)).is_err() {
                            self.model.set_notice("not connected: cannot change model");
                        }
                    } else {
                        self.model.set_notice("not connected: cannot change model");
                    }
                }
                self.request_redraw();
                return true;
            }
            self.model.model_picker.close();
            self.request_redraw();
            return true;
        }

        if !on_button {
            return false;
        }
        self.toggle_model_picker();
        true
    }

    /// Track both the caption button and the rows in its menu. Returns whether
    /// painting state changed.
    pub(crate) fn model_picker_hover(&mut self, x: f64, y: f64) -> bool {
        let button = self.has_model_caption() && self.frame.hits_model_button(x, y);
        let mut changed = self.model.model_picker.set_button_hover(button);
        let row = self
            .model
            .model_picker
            .is_open()
            .then(|| {
                self.frame
                    .model_menu_row_at(self.model.model_picker.visual_rows(), x, y)
            })
            .flatten();
        changed |= self.model.model_picker.set_hover(row);
        changed
    }
}
